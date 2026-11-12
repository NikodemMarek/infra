{
  writeShellApplication,
  jq,
  jw,
  git,
  coreutils,
}:

writeShellApplication {
  name = "claude-statusline";
  # jj is not here: jw takes it from the user's PATH.
  runtimeInputs = [
    jq
    jw
    git
    coreutils
  ];
  # Best-effort output: no errexit/pipefail, so a failing segment is skipped instead of aborting the line.
  bashOptions = [ "nounset" ];
  text = builtins.readFile ./statusline.sh;
  meta.mainProgram = "claude-statusline";
}
