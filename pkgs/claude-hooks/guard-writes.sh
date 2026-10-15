# PreToolUse guard for Edit/Write/NotebookEdit in subagents.
# Usage: claude-guard-writes code    -> allow writes only inside the session cwd or ~/projects, never into config/secrets/.git/architect memory
#        claude-guard-writes memory  -> allow writes only to the architect's own files in the memory repo
#                                       (README.md, INDEX.md, templates/**, projects/<slug>/{overview,decisions,log}.md, projects/<slug>/notes/**)
# (writeShellApplication prepends the shebang and `set -o errexit -o nounset -o pipefail`)
MODE="${1:?mode required}"
CFG="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
MEM="${CLAUDE_MEMORY_DIR:-$CFG/memory}"
input="$(cat)"
path="$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' <<<"$input")"
cwd="$(jq -r '.cwd // empty' <<<"$input")"
[ -z "$cwd" ] && cwd="$PWD"

deny() {
  jq -n --arg r "$1" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
  exit 0
}

[ -z "$path" ] && deny "No file path in tool input."
case "$path" in /*) ;; *) path="$cwd/$path" ;; esac
abs="$(realpath -m -- "$path")"
under() { case "$abs/" in "$(realpath -m -- "$1")"/*) return 0 ;; esac; return 1; }

case "$abs" in */.git/*|*/.git) deny "Writing inside .git is not allowed for this agent." ;; esac
case "$abs" in */.jj/*|*/.jj) deny "Writing inside .jj is not allowed for this agent." ;; esac

if [ "$MODE" = memory ]; then
  under "$MEM" || deny "Architect may only write inside $MEM (got $abs)."
  # The curator owns the rest of the repo (KNOWLEDGE.md, SCOPES.toml, global/, lang/, topic/, org/, projects/<slug>/facts/)
  # and private/ holds credentials: only the architect's own files are allowed.
  rel="${abs#"$(realpath -m -- "$MEM")"/}"
  slug='[a-z0-9][a-z0-9-]{0,63}'
  [[ "$rel" =~ ^(README\.md|INDEX\.md|templates/.+)$ ]] && exit 0
  [[ "$rel" =~ ^projects/$slug/(overview\.md|decisions\.md|log\.md|notes/.+)$ ]] && exit 0
  deny "Architect may only write README.md, INDEX.md, templates/**, projects/<slug>/{overview,decisions,log}.md and projects/<slug>/notes/** in $MEM (got $rel); the rest is the curator's or private."
fi

# MODE=code
# Other repos under ~/projects are fine, but not the other workspaces of this one (<repo>, <repo>.agents/*):
# the agent's work must stay in its own workspace to be squashed into the right change.
home="$cwd"
case "$(dirname -- "$cwd")" in *.agents) home="$(dirname -- "$cwd")" && home="${home%.agents}" ;; esac
under "$cwd" || { under "$HOME/projects" && ! under "$home" && ! under "$home.agents"; } \
  || deny "Writes outside the working directory ($cwd) and other repos under ~/projects are not allowed (got $abs)."
for p in "$HOME/.claude" "$CFG" "$HOME/.ssh" "$HOME/.gnupg" "$HOME/.config" "$HOME/.local" "$HOME/.aws" "$HOME/.kube" "$HOME/.m2/settings.xml" "$MEM"; do
  under "$p" && deny "Path $abs is protected (config/secrets/architect memory)."
done
# Top-level dotfiles in $HOME (.bashrc, .profile, ...)
[ "$(dirname -- "$abs")" = "$HOME" ] && case "$(basename -- "$abs")" in .*) deny "Home dotfiles are protected." ;; esac
case "$(basename -- "$abs")" in .env|.env.*|*.pem|*.key|id_rsa*|id_ed25519*) deny "Secret-like files are protected." ;; esac
exit 0
