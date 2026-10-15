{
  writeShellApplication,
  knowledge,
  claude,
  jujutsu,
}:

# event handler for the knowledge curator: `knowledge.submit` stores a
# submission, `knowledge.curate[.weekly]` (from the timers) run the curator.
# `knowledge.review_needed` is not handled here: the router sends it to notify.
writeShellApplication {
  name = "event-handler-knowledge";
  runtimeInputs = [ knowledge ];
  text = ''
    event=$(cat)
    # the user's own jj, if any, comes first on PATH: ours is appended below
    cargo_bin="''${CARGO_HOME:-$HOME/.cargo}/bin"
    if [[ -x "$cargo_bin/jj" ]]; then export PATH="$cargo_bin:$PATH"; fi
    export PATH="$PATH:${jujutsu}/bin"
    export KNOWLEDGE_CLAUDE=${claude}/bin/claude-bare
    export KNOWLEDGE_SKILLS_DIR=${claude.config}/skills
    case "''${ER_TYPE:-}" in
      knowledge.submit) knowledge receive <<<"$event" ;;
      knowledge.curate) knowledge curate --if-idle ;;
      knowledge.curate.weekly) knowledge curate --weekly ;;
      *) echo "event-handler-knowledge: unexpected type ''${ER_TYPE:-}" >&2; exit 2 ;;
    esac
  '';
}
