{
  lib,
  writeShellApplication,
  coreutils,
  jw,
  session-message,
  name,
  subcommand,
}:
# event handler: an event becomes a session message via `jw <subcommand>`, which decides
# whether and what to tell which session. The event text is untrusted: jw frames it.
writeShellApplication {
  name = "event-handler-${name}";
  runtimeInputs = [coreutils];
  text = ''
    event=$(cat)

    out=$(${lib.getExe jw} ${subcommand} <<<"$event") || {
      echo "${name}: cannot render ''${ER_TYPE:-?} event" >&2
      exit 1
    }
    [[ -n $out ]] || exit 0

    ${lib.getExe session-message} <<<"$out"
  '';
}
