use super::*;

fn claude(
    pid: u32,
    cwd: &str,
    agent: Option<&str>,
    name: Option<&str>,
    socket: Option<&str>,
) -> Claude {
    Claude {
        pid,
        cwd: cwd.to_string(),
        agent: agent.map(str::to_string),
        status: None,
        waiting_for: None,
        status_updated_at: None,
        session_id: None,
        name: name.map(str::to_string),
        messaging_socket: socket.map(str::to_string),
    }
}

#[test]
fn matches_registry_name() {
    let c = vec![claude(1, "/w/x", None, Some("fix"), Some("/s/1"))];
    assert_eq!(
        message_sessions(c, "fix"),
        vec![(1, "fix".to_string(), "/s/1".to_string())]
    );
}

#[test]
fn matches_cwd_basename() {
    let c = vec![claude(
        1,
        "/home/u/r.agents/w1",
        None,
        Some("other"),
        Some("/s/1"),
    )];
    assert_eq!(
        message_sessions(c, "w1"),
        vec![(1, "other".to_string(), "/s/1".to_string())]
    );
}

#[test]
fn matches_agent_and_labels_with_agent_when_name_empty_or_absent() {
    let c = vec![
        claude(1, "/w/a", Some("jw-1"), Some(""), Some("/s/1")),
        claude(2, "/w/b", Some("jw-1"), None, Some("/s/2")),
    ];
    assert_eq!(
        message_sessions(c, "jw-1"),
        vec![
            (1, "jw-1".to_string(), "/s/1".to_string()),
            (2, "jw-1".to_string(), "/s/2".to_string()),
        ]
    );
}

#[test]
fn label_is_question_mark_without_name_or_agent() {
    let c = vec![claude(1, "/w/w1", None, None, Some("/s/1"))];
    assert_eq!(
        message_sessions(c, "w1"),
        vec![(1, "?".to_string(), "/s/1".to_string())]
    );
}

#[test]
fn registry_name_wins_over_agent_in_label() {
    let c = vec![claude(1, "/w/a", Some("jw-1"), Some("nm"), Some("/s/1"))];
    assert_eq!(
        message_sessions(c, "jw-1"),
        vec![(1, "nm".to_string(), "/s/1".to_string())]
    );
}

#[test]
fn skips_sessions_without_socket() {
    let c = vec![
        claude(1, "/w/w1", Some("w1"), Some("w1"), None),
        claude(2, "/w/w1", Some("w1"), Some("w1"), Some("/s/2")),
    ];
    let found = message_sessions(c, "w1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 2);
}

#[test]
fn no_match_is_empty() {
    let c = vec![claude(1, "/w/a", Some("x"), Some("y"), Some("/s/1"))];
    assert!(message_sessions(c, "z").is_empty());
    assert!(message_sessions(vec![], "z").is_empty());
}

#[test]
fn name_must_match_whole_value() {
    let c = vec![claude(1, "/w/abc", Some("abc"), Some("abc"), Some("/s/1"))];
    assert!(message_sessions(c, "ab").is_empty());
}

#[test]
fn several_matches_all_returned() {
    let c = vec![
        claude(1, "/w/a", Some("t"), Some("n1"), Some("/s/1")),
        claude(2, "/w/t", None, Some("n2"), Some("/s/2")),
        claude(3, "/w/c", None, Some("t"), Some("/s/3")),
    ];
    let pids: Vec<u32> = message_sessions(c, "t").iter().map(|m| m.0).collect();
    assert_eq!(pids, vec![1, 2, 3]);
}
