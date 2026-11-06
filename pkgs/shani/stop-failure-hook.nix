{
  writeShellApplication,
  jq,
  claude-hooks,
}:
# StopFailure hook (a turn ended on an API error): sessions with a record (AGENT_SESSION)
# post claude.failed with the error; never fails.
writeShellApplication {
  name = "claude-stop-failure-hook";
  runtimeInputs = [jq claude-hooks];
  text = ''
    input=$(cat)
    [ -n "''${AGENT_SESSION:-}" ] || exit 0
    jq -c '{title: .error_type, body: .error_message}' <<<"$input" | claude-route-event claude.failed >&2 || true
    exit 0
  '';
}
