use super::*;

#[path = "../common/nats.rs"]
#[allow(clippy::duplicate_mod)]
mod nats;

// One test: it sets the process-wide AGENT_SESSION.
#[test]
fn track_worktree_behaviour() {
    let server = nats::NatsServer::start();
    let records = Records::connect(&server.url()).unwrap();
    let base = fs::canonicalize(env::temp_dir())
        .unwrap()
        .join(format!("jw-session-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    env::remove_var("AGENT_SESSION");
    let dir = |n: &str| {
        let d = base.join(n);
        fs::create_dir_all(&d).unwrap();
        d
    };
    let s = |p: &Path| p.to_str().unwrap().to_string();
    let wts = || {
        records
            .read("s1")
            .unwrap()
            .map(|r| r.worktrees)
            .unwrap_or_default()
    };

    let plain = dir("plain");
    assert_eq!(
        track_worktree(&records, &s(&plain)),
        Err("not in an agent session".to_string())
    );
    env::set_var("AGENT_SESSION", "");
    assert_eq!(
        track_worktree(&records, &s(&plain)),
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
            track_worktree(&records, &bad),
            Err(format!("not an absolute directory: {bad}"))
        );
    }

    records
        .update("s1", |r| {
            r.title = Some("Keep".into());
            r.status = Some("busy".into());
            r.workspace = Some("/ws".into());
            true
        })
        .unwrap();
    let (a, b, c) = (dir("wt/a"), dir("wt/b"), dir("wt/c"));
    for d in [&b, &a, &b, &a] {
        track_worktree(&records, &s(d)).unwrap();
    }
    assert_eq!(wts(), vec![s(&b), s(&a)]);
    let r = records.read("s1").unwrap().unwrap();
    assert_eq!(r.title.as_deref(), Some("Keep"));
    assert_eq!(r.status.as_deref(), Some("busy"));
    assert_eq!(r.workspace.as_deref(), Some("/ws"));

    fs::remove_dir_all(&a).unwrap();
    track_worktree(&records, &s(&c)).unwrap();
    assert_eq!(wts(), vec![s(&b), s(&c)]);

    let real = dir("real");
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    track_worktree(&records, &s(&link)).unwrap();
    track_worktree(&records, &s(&real)).unwrap();
    assert_eq!(wts(), vec![s(&b), s(&c), s(&real)]);

    let _ = fs::remove_dir_all(&base);
}
