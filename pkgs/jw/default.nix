# `jw`: the python script does the work; the fish function cds where it says.
# fzf, shpool, bash and knowledge are pinned; jj and claude come from the user's PATH,
# and so does claude-bare, which knowledge uses.
{
  lib,
  runCommand,
  python3,
  fzf,
  shpool,
  bash,
  knowledge,
  projectsRoot ? "~/projects",
}:
assert lib.assertMsg (builtins.match "[~/][A-Za-z0-9._/-]*" projectsRoot != null) "jw: projectsRoot must be ~/... or an absolute path of [A-Za-z0-9._/-]";
  runCommand "jw" {} ''
    install -Dm755 ${./jw.py} $out/bin/jw
    substituteInPlace $out/bin/jw \
      --replace-fail "#!/usr/bin/env python3" "#!${python3}/bin/python3" \
      --replace-fail 'FZF = "fzf"' 'FZF = "${fzf}/bin/fzf"' \
      --replace-fail 'SHPOOL = "shpool"' 'SHPOOL = "${shpool}/bin/shpool"' \
      --replace-fail 'BASH = "bash"' 'BASH = "${bash}/bin/bash"' \
      --replace-fail 'KNOWLEDGE = "knowledge"' 'KNOWLEDGE = "${knowledge}/bin/knowledge"' \
      --replace-fail 'ROOT = os.path.expanduser("~/projects")' 'ROOT = os.path.expanduser("${projectsRoot}")'
    install -Dm644 ${./jw.fish} $out/share/fish/vendor_functions.d/jw.fish
    install -Dm644 ${./completions.fish} $out/share/fish/vendor_completions.d/jw.fish
    substituteInPlace $out/share/fish/vendor_completions.d/jw.fish \
      --replace-fail '~/projects/*/.jj' '${projectsRoot}/*/.jj'
  ''
