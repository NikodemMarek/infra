use crate::cmd::{run, Result};

pub fn config(key: &str) -> Result<Option<String>> {
    let p = run(&["jj", "config", "get", key], false)?;
    let value = p.stdout.trim();
    Ok((p.ok && !value.is_empty()).then(|| value.to_string()))
}

pub fn root() -> Result<()> {
    run(&["jj", "--no-pager", "--color=never", "root"], true)?;
    Ok(())
}

pub fn remote_url(remote: &str) -> Result<Option<String>> {
    let out = run(
        &["jj", "--no-pager", "--color=never", "git", "remote", "list"],
        true,
    )?;
    let mut url = None;
    for line in out.stdout.lines() {
        let (name, u) = line.split_once(' ').unwrap_or((line, ""));
        if name == remote {
            url = Some(u.trim().to_string());
        }
    }
    Ok(url.filter(|u| !u.is_empty()))
}
