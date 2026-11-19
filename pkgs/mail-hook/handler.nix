{
  writeShellApplication,
  jq,
  natscli,
  coreutils,
}:
writeShellApplication {
  name = "event-handler-mail-matrix";
  runtimeInputs = [
    jq
    natscli
    coreutils
  ];
  text = ''
    jq -c -f ${./to-matrix.jq} | timeout 5 nats --server "''${NATS_URL:?}" pub --no-templates matrix.send
  '';
}
