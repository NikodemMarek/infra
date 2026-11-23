{
  lib,
  writeShellApplication,
  coreutils,
  jw,
  session-message,
}:
# event handler: a forge.* event becomes a session message via `jw forge-notice`, which decides
# whether and what to tell the session. The forge text is untrusted: jw frames it.
writeShellApplication {
  name = "event-handler-forge";
  runtimeInputs = [coreutils];
  text = ''
    event=$(cat)

    out=$(${lib.getExe jw} forge-notice <<<"$event") || {
      echo "forge: cannot render ''${ER_TYPE:-?} event" >&2
      exit 1
    }
    [[ -n $out ]] || exit 0

    ${lib.getExe session-message} <<<"$out"
  '';
}
