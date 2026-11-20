{
  lib,
  runCommand,
}: let
  skills = lib.filterAttrs (_: t: t == "directory") (builtins.readDir ./skills);
in
  # $out/<name>/SKILL.md for every shared skill, ready to be linked into the
  # skills directory of any tool that reads the Agent Skills format.
  runCommand "agent-skills" {} ''
    mkdir -p "$out"
    ${lib.concatMapStringsSep "\n" (n: ''cp -rL "${./skills + "/${n}"}" "$out/${n}"'') (
      lib.attrNames skills
    )}
  ''
