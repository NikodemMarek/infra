{
  writeShellApplication,
  jq,
  natscli,
}:
# event handler: republishes an event of this pod as `remote.notify.<host>` on the cluster hub,
# the only subject yenn's leaf lets through; yenn's remote-notify sanitises it into a notification
writeShellApplication {
  name = "event-handler-forward-notify";
  runtimeInputs = [jq natscli];
  text = ''
    project=$(uname -n)
    jq -c --arg type "''${ER_TYPE:-}" --arg project "$project" '
      {type: "remote.notify", project: $project, title: "\((.session // .shpool // "") + " " + $type | ltrimstr(" ")): \(.title // "")", body: (.body // .reason)}
    ' | timeout 4 nats --server "''${NATS_URL:-nats://127.0.0.1:4222}" pub --no-templates "remote.notify.$project"
  '';
}
