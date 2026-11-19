use std::collections::{BTreeMap, HashMap, HashSet};

use crate::forge::{make_forge, strip_stack, Forge, Mr};
use crate::jj::{
    choose_bookmark, jj, load_commits, log_field, log_ids, remote_branches, trunc, trunk_branch,
    Commit, Result, DEFAULT_REVSET, REMOTE_TITLE,
};

fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or_default()
}

fn names_of(commits: &[Commit], prefix: &str) -> HashMap<String, String> {
    commits
        .iter()
        .filter(|c| !c.bookmarks.is_empty())
        .map(|c| (c.change.clone(), choose_bookmark(c, prefix).0))
        .collect()
}

pub fn pull_descriptions(
    commits: &[Commit],
    fg: &dyn Forge,
    prefix: &str,
    pushed: &BTreeMap<String, String>,
    dry_run: bool,
) -> Result<bool> {
    let mut pulled = false;
    for c in commits {
        let (branch, _) = choose_bookmark(c, prefix);
        let mr = if pushed.contains_key(&branch) {
            fg.find(&branch)?
        } else {
            None
        };
        let Some(mr) = mr.filter(|m| m.state != "merged") else {
            continue;
        };
        let mut remote_desc = mr.title.trim().to_string();
        let body = strip_stack(&mr.description);
        if !body.is_empty() {
            remote_desc.push_str("\n\n");
            remote_desc.push_str(&body);
        }
        if remote_desc != c.description.trim() {
            eprintln!(
                "  pulled title/description of {} into {}",
                mr.reference,
                c.short()
            );
            if !dry_run {
                jj(&["describe", "-r", &c.change, "-m", &remote_desc])?;
                pulled = true;
            }
        }
    }
    Ok(pulled)
}

fn merged_bottom(
    remote: &str,
    prefix: &str,
    commits: &[Commit],
    names: &HashMap<String, String>,
    branch: &HashMap<String, (String, String)>,
) -> Vec<Commit> {
    let lookup = || -> Result<Vec<Commit>> {
        let mut merged = Vec::new();
        let mut fg = make_forge(remote)?;
        for c in commits {
            let name = match branch.get(&c.change) {
                Some((b, _)) => Some(b.as_str()),
                None => names.get(&c.change).map(String::as_str),
            };
            let mr = find_mr(&mut *fg, c, name, prefix)?;
            let state = change_state(c, mr.as_ref())?;
            if let Some(mr) = &mr {
                if state == "changed" {
                    eprintln!(
                        "warning: {} is merged but {} has changes since its last push (or the pushed commit is unknown here); keeping it",
                        mr.reference,
                        c.short()
                    );
                } else if state == "elsewhere" {
                    eprintln!(
                        "warning: {} is merged but not into {} here; keeping it",
                        mr.reference,
                        trunk_branch(remote)?
                    );
                }
            }
            if state != "merged" {
                break;
            }
            merged.push(c.clone());
        }
        Ok(merged)
    };
    lookup().unwrap_or_else(|e| {
        let message = e.split_whitespace().collect::<Vec<_>>().join(" ");
        eprintln!("warning: cannot look up merged MRs, not dropping any: {message}");
        vec![]
    })
}

pub fn find_mr(
    fg: &mut dyn Forge,
    c: &Commit,
    branch: Option<&str>,
    prefix: &str,
) -> Result<Option<Mr>> {
    let branch = branch.filter(|b| !b.is_empty());
    if let Some(mr) = branch.map(|b| fg.find(b)).transpose()?.flatten() {
        return Ok(Some(mr));
    }
    let suffix = format!("-{}", c.short());
    if let Some(mr) = fg
        .recent()?
        .iter()
        .find(|m| m.source.starts_with(prefix) && m.source.ends_with(&suffix))
    {
        return Ok(Some(mr.clone()));
    }
    if c.description.is_empty() && c.bookmarks.is_empty() {
        return Ok(None);
    }
    let slug = choose_bookmark(c, prefix).0;
    if Some(slug.as_str()) == branch {
        return Ok(None);
    }
    fg.find(&slug)
}

pub fn change_state(c: &Commit, mr: Option<&Mr>) -> Result<String> {
    let Some(mr) = mr else {
        return Ok("no-mr".to_string());
    };
    if mr.state != "merged" {
        return Ok(mr.state.clone());
    }
    let unchanged = match mr.head.as_deref() {
        Some(head) if !log_ids(&format!("present({head})"))?.is_empty() => {
            head == c.commit
                || jj(&["interdiff", "--from", head, "--to", &c.commit, "--summary"])?
                    .trim()
                    .is_empty()
        }
        _ => false,
    };
    if !unchanged {
        return Ok("changed".to_string());
    }
    for s in mr
        .landed
        .iter()
        .map(String::as_str)
        .chain(mr.head.as_deref())
    {
        if !log_ids(&format!("present({s}) & ::trunk()"))?.is_empty() {
            return Ok("merged".to_string());
        }
    }
    Ok("elsewhere".to_string())
}

fn stack_lines(remote: &str, prefix: &str) -> Result<(Vec<String>, bool)> {
    let before = load_commits(DEFAULT_REVSET)?;
    if before.is_empty() {
        return Ok((vec![], true));
    }
    let names = names_of(&before, prefix);
    jj(&["git", "fetch"])?;
    let commits = load_commits(DEFAULT_REVSET)?;
    let mut fg = make_forge(remote)?;
    let mut lines = Vec::new();
    let mut merged = true;
    for c in &commits {
        let mr = find_mr(
            &mut *fg,
            c,
            names.get(&c.change).map(String::as_str),
            prefix,
        )?;
        let state = change_state(c, mr.as_ref())?;
        merged &= state == "merged";
        let title = first_line(&c.description).replace(['\t', '\r', '\n'], " ");
        lines.push(format!(
            "{}\t{state}\t{}\t{title}",
            c.short(),
            mr.as_ref().map_or("-", |m| m.url.as_str())
        ));
    }
    Ok((lines, merged))
}

pub fn stack_status(remote: &str, prefix: &str) -> i32 {
    match stack_lines(remote, prefix) {
        Err(e) => {
            println!("unknown");
            eprintln!("error: {e}");
            2
        }
        Ok((lines, merged)) => {
            for line in lines {
                println!("{line}");
            }
            if merged {
                println!("merged");
                0
            } else {
                println!("not merged");
                1
            }
        }
    }
}

pub fn sync(remote: &str, prefix: &str, revset: &str) -> Result<Vec<String>> {
    let before = load_commits(revset)?;
    let names = names_of(&before, prefix);
    jj(&["git", "fetch"])?;
    let tips = remote_branches(remote)?;
    let mut branch: HashMap<String, (String, String)> = HashMap::new();
    for c in &before {
        let known = names.get(&c.change).filter(|b| tips.contains_key(*b));
        let found = match known {
            Some(b) => Some(b.clone()),
            None => {
                let suffix = format!("-{}", c.short());
                tips.keys()
                    .find(|n| n.starts_with(prefix) && n.ends_with(&suffix))
                    .cloned()
            }
        };
        if let Some(b) = found {
            let tip = tips[&b].clone();
            branch.insert(c.change.clone(), (b, tip));
        }
    }

    if !branch.is_empty() {
        let remote_tips = before
            .iter()
            .filter_map(|c| branch.get(&c.change))
            .map(|(_, r)| r.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        let mine = before
            .iter()
            .map(|c| c.commit.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        let revived = format!("((::({remote_tips}) ~ ::trunk() ~ ::({mine})) & ::visible_heads())");
        if !log_ids(&revived)?.is_empty() {
            jj(&["abandon", &revived])?;
        }
    }

    let dropped = merged_bottom(remote, prefix, &before, &names, &branch);
    let mut rebase = vec!["rebase", "-b", "@"];
    if !dropped.is_empty() {
        for c in &dropped {
            eprintln!("merged: {} {}", c.short(), first_line(&c.description));
        }
        let all = dropped
            .iter()
            .map(|c| c.commit.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        let landed: HashSet<String> = log_ids(&format!("::trunk() & ({all})"))?
            .into_iter()
            .collect();
        let gone: Vec<&str> = dropped
            .iter()
            .map(|c| c.commit.as_str())
            .filter(|c| !landed.contains(*c))
            .collect();
        if !gone.is_empty() {
            let mut abandon = vec!["abandon"];
            abandon.extend(gone);
            jj(&abandon)?;
        }
        if !log_ids("@ & ::trunk()")?.is_empty() {
            jj(&["new", "trunk()"])?;
        }
        if dropped.len() < before.len() {
            rebase.extend(["-b", before[dropped.len()].change.as_str()]);
        }
    }
    rebase.extend(["-o", "trunk()", "--skip-emptied"]);
    jj(&rebase)?;

    let mut failures = Vec::new();
    let mut current = load_commits(revset)?;
    current.reverse();
    for c in &current {
        let Some((b, r)) = branch.get(&c.change) else {
            continue;
        };
        let (l, l8) = (c.commit.as_str(), c.short());
        jj(&["bookmark", "set", b, "-r", l, "--allow-backwards"])?;
        if r == l {
            continue;
        }
        let marker = format!(r#"description(substring:"{REMOTE_TITLE}{b}")"#);
        if let Some(left) = log_field(&format!("{l}:: & {marker}"), "change_id")?.first() {
            failures.push(format!(
                "unresolved {} for {b}; squash or abandon it first",
                trunc(left, 8)
            ));
            continue;
        }
        let remote_only = log_ids(&format!("::{r} ~ ::trunk()"))?;
        if remote_only.is_empty() {
            continue;
        }
        let evolog = jj(&[
            "evolog",
            "-r",
            l,
            "--no-graph",
            "-T",
            r#"commit.commit_id() ++ "\n""#,
        ])?;
        let evo: HashSet<&str> = evolog.split_whitespace().collect();
        let x = remote_only.iter().find(|i| evo.contains(i.as_str()));
        if x.is_some_and(|x| x == r) {
            continue;
        }

        let base = x.map_or(l, String::as_str);
        let r12 = trunc(r, 12);
        let what = if x.is_some() {
            format!("Commits on {remote}/{b} that are not in this change, squashed (remote {r12}).")
        } else {
            format!(
                "{remote}/{b} shares no version with this change; this is the whole-tree \
                 difference to it (remote {r12}) and may revert newer trunk changes."
            )
        };
        jj(&[
            "new",
            base,
            "--no-edit",
            "-m",
            &format!("{REMOTE_TITLE}{b}\n\n{what}"),
        ])?;
        let t = log_field(&format!("latest(children({base}) & {marker})"), "change_id")?
            .into_iter()
            .next()
            .ok_or("cannot find the new remote changes commit")?;
        jj(&["restore", "--from", r, "--into", &t])?;
        if base != l {
            jj(&["rebase", "-r", &t, "-o", l])?;
            jj(&["abandon", base])?;
        }

        let t8 = trunc(&t, 8);
        if log_field(&t, r#"if(empty, "1", "0")"#)? == ["1"] {
            jj(&["abandon", &t])?;
            continue;
        }
        let conflicted = log_field(&t, r#"if(conflict, "1", "0")"#)? == ["1"];
        if let Some(x) = x {
            if !conflicted
                && jj(&["interdiff", "--from", x, "--to", l, "--summary"])?
                    .trim()
                    .is_empty()
            {
                jj(&["squash", "--from", &t, "--into", l, "-u"])?;
                eprintln!("pulled remote commits of {b} into {l8}");
                continue;
            }
        }
        jj(&["rebase", "-r", &t, "-A", l])?;
        let msg = if x.is_some() {
            format!(
                "{b}: the remote has commits not in {l8}; they are in {t8} after it. \
                 Take them: `jj squash -r {t8} -u`. \
                 Discard them: `jj abandon {t8}; jj git push --remote {remote} -b exact:{b}` \
                 (overwrites the remote)."
            )
        } else {
            format!(
                "{b}: {remote}/{b} differs and shares no version with {l8} (rewritten on the \
                 forge, or amended by someone else). Inspect {t8} (whole-tree difference, may \
                 revert newer trunk). Move what you need into {l8} by hand, then \
                 `jj abandon {t8}` and push with `jj git push --remote {remote} -b exact:{b}`."
            )
        };
        failures.push(msg + if conflicted { " (has conflicts)" } else { "" });
    }
    Ok(failures)
}
