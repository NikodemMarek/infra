use super::*;

// One test: it sets process-wide env vars.
#[test]
fn track_worktree_behaviour() {
    let base = fs::canonicalize(env::temp_dir())
        .unwrap()
        .join(format!("jw-session-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    env::set_var("XDG_STATE_HOME", base.join("state"));
    env::remove_var("AGENT_SESSION");
    env::remove_var("AGENT_ORIGIN");
    let dir = |n: &str| {
        let d = base.join(n);
        fs::create_dir_all(&d).unwrap();
        d
    };
    let s = |p: &Path| p.to_str().unwrap().to_string();
    let wts = || {
        Records::new()
            .read("s1")
            .map(|r| r.worktrees)
            .unwrap_or_default()
    };

    let plain = dir("plain");
    assert_eq!(
        track_worktree(&s(&plain)),
        Err("not in an agent session".to_string())
    );
    env::set_var("AGENT_SESSION", "");
    assert_eq!(
        track_worktree(&s(&plain)),
        Err("not in an agent session".to_string())
    );

    env::set_var("AGENT_SESSION", "s1");
    fs::write(base.join("afile"), "x").unwrap();
    for bad in [
        "rel/dir".to_string(),
        s(&base.join("missing")),
        s(&base.join("afile")),
    ] {
        assert_eq!(
            track_worktree(&bad),
            Err(format!("not an absolute directory: {bad}"))
        );
    }
    assert!(!base.join("state/agent-sessions/s1.json").exists());

    Records::new()
        .update("s1", |r| {
            r.title = Some("Keep".into());
            r.status = Some("busy".into());
            r.workspace = Some("/ws".into());
            true
        })
        .unwrap();
    let (a, b, c) = (dir("wt/a"), dir("wt/b"), dir("wt/c"));
    for d in [&b, &a, &b, &a] {
        track_worktree(&s(d)).unwrap();
    }
    assert_eq!(wts(), vec![s(&b), s(&a)]);
    let r = Records::new().read("s1").unwrap();
    assert_eq!(r.title.as_deref(), Some("Keep"));
    assert_eq!(r.status.as_deref(), Some("busy"));
    assert_eq!(r.workspace.as_deref(), Some("/ws"));

    fs::remove_dir_all(&a).unwrap();
    track_worktree(&s(&c)).unwrap();
    assert_eq!(wts(), vec![s(&b), s(&c)]);

    let real = dir("real");
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    track_worktree(&s(&link)).unwrap();
    track_worktree(&s(&real)).unwrap();
    assert_eq!(wts(), vec![s(&b), s(&c), s(&real)]);

    env::set_var("AGENT_ORIGIN", "event:gitlab-note");
    let e = dir("ev");
    track_worktree(&s(&e)).unwrap();
    assert_eq!(wts().last(), Some(&s(&e)));

    let _ = fs::remove_dir_all(&base);
}
