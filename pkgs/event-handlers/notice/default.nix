{
  lib,
  writeShellApplication,
  coreutils,
  session-message,
  name,
  command,
}:
# event handler: an event becomes a session message via `command`, which decides
# whether and what to tell which session. The event text is untrusted: the command frames it.
writeShellApplication {
  name = "event-handler-${name}";
  runtimeInputs = [coreutils];
  text = ''
    event=$(cat)

    out=$(${command} <<<"$event") || {
      echo "${name}: cannot render ''${ER_TYPE:-?} event" >&2
      exit 1
    }
    [[ -n $out ]] || exit 0

    ${lib.getExe session-message} <<<"$out"
  '';
}
