use super::{idle_sessions, Claude, IDLE_LIMIT};
use std::collections::HashMap;

const NOW: f64 = 1_000_000.0;

fn claude(pid: u32, agent: Option<&str>, status: &str, age: f64) -> Claude {
    Claude {
        pid,
        cwd: format!("/w/{pid}"),
        agent: agent.map(str::to_string),
        status: Some(status.to_string()),
        waiting_for: None,
        status_updated_at: Some((NOW - age) * 1000.0),
        session_id: Some(format!("sid-{pid}")),
        name: None,
        messaging_socket: None,
    }
}

fn state(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn names(found: &[(String, String, String)]) -> Vec<&str> {
    found.iter().map(|f| f.0.as_str()).collect()
}

#[test]
fn idle_detached_agent_qualifies() {
    let c = [claude(1, Some("a"), "idle", IDLE_LIMIT + 1.0)];
    let found = idle_sessions(&c, &state(&[("a", "disconnected")]), NOW);
    assert_eq!(found, vec![("a".into(), "/w/1".into(), "sid-1".into())]);
}

#[test]
fn idle_exactly_at_limit_qualifies() {
    let c = [claude(1, Some("a"), "idle", IDLE_LIMIT)];
    assert_eq!(
        idle_sessions(&c, &state(&[("a", "disconnected")]), NOW).len(),
        1
    );
}

#[test]
fn idle_below_limit_does_not_qualify() {
    let c = [claude(1, Some("a"), "idle", IDLE_LIMIT - 1.0)];
    assert!(idle_sessions(&c, &state(&[("a", "disconnected")]), NOW).is_empty());
}

#[test]
fn idle_needs_disconnected_state() {
    let c = [
        claude(1, Some("a"), "idle", IDLE_LIMIT + 1.0),
        claude(2, Some("b"), "idle", IDLE_LIMIT + 1.0),
        claude(3, Some("c"), "idle", IDLE_LIMIT + 1.0),
    ];
    let st = state(&[("a", "attached"), ("b", "other")]);
    assert!(idle_sessions(&c, &st, NOW).is_empty());
}

#[test]
fn idle_busy_member_blocks_group() {
    let c = [
        claude(1, Some("a"), "idle", IDLE_LIMIT + 1.0),
        claude(2, Some("a"), "busy", IDLE_LIMIT + 1.0),
    ];
    assert!(idle_sessions(&c, &state(&[("a", "disconnected")]), NOW).is_empty());
}

#[test]
fn idle_recent_member_blocks_group() {
    let c = [
        claude(1, Some("a"), "idle", IDLE_LIMIT + 1.0),
        claude(2, Some("a"), "idle", 10.0),
    ];
    assert!(idle_sessions(&c, &state(&[("a", "disconnected")]), NOW).is_empty());
}

#[test]
fn idle_missing_status_time_does_not_qualify() {
    let mut one = claude(1, Some("a"), "idle", IDLE_LIMIT + 1.0);
    one.status_updated_at = None;
    assert!(idle_sessions(&[one], &state(&[("a", "disconnected")]), NOW).is_empty());
}

#[test]
fn idle_permission_prompt_counts_as_stuck() {
    let mut waiting = claude(1, Some("a"), "waiting", IDLE_LIMIT + 1.0);
    waiting.waiting_for = Some("permission prompt".into());
    let mut other = claude(2, Some("b"), "waiting", IDLE_LIMIT + 1.0);
    other.waiting_for = Some("input".into());
    let none = claude(3, Some("c"), "waiting", IDLE_LIMIT + 1.0);
    let st = state(&[
        ("a", "disconnected"),
        ("b", "disconnected"),
        ("c", "disconnected"),
    ]);
    let found = idle_sessions(&[waiting, other, none], &st, NOW);
    assert_eq!(names(&found), vec!["a"]);
}

#[test]
fn idle_ignores_claudes_without_agent() {
    let c = [claude(1, None, "idle", IDLE_LIMIT + 1.0)];
    let st = state(&[("a", "disconnected")]);
    assert!(idle_sessions(&c, &st, NOW).is_empty());
}

#[test]
fn idle_group_uses_first_member_and_sorts_by_name() {
    let mut first = claude(1, Some("b"), "idle", IDLE_LIMIT + 1.0);
    first.session_id = None;
    let c = [
        first,
        claude(2, Some("a"), "idle", IDLE_LIMIT + 1.0),
        claude(3, Some("b"), "idle", IDLE_LIMIT + 5.0),
    ];
    let st = state(&[("a", "disconnected"), ("b", "disconnected")]);
    let found = idle_sessions(&c, &st, NOW);
    assert_eq!(
        found,
        vec![
            ("a".into(), "/w/2".into(), "sid-2".into()),
            ("b".into(), "/w/1".into(), "?".into()),
        ]
    );
}
