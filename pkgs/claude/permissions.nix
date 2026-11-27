# Claude Code permission rules (https://code.claude.com/docs/en/permissions),
# imported by settings.nix as `permissions`. `@memoryDir@`, `@projectsDir@` and
# `@knowledgeFile@` are substituted at build time.
{
  allow = [
    "Bash(jj log:*)"
    "Bash(jj status:*)"
    "Bash(jj show:*)"
    "Bash(jj diff:*)"
    "Bash(jj evolog:*)"
    "Bash(jj op log:*)"
    "Bash(jj op show:*)"
    "Bash(jj workspace list:*)"
    "Bash(jj bookmark list:*)"
    "Bash(jj file show:*)"
    "Bash(jj file list:*)"
    "Bash(jj config list:*)"
    "Bash(jj new:*)"
    "Bash(jj squash:*)"
    "Bash(jj split:*)"
    "Bash(jj edit:*)"
    "Bash(jj describe:*)"
    "Bash(jj commit:*)"
    "Bash(jj rebase:*)"
    "Bash(jj upload:*)"
    "Bash(jj stack:*)"
    "Bash(jw agent -d:*)"
    "Bash(jw msg:*)"
    "Bash(jw session:*)"
    "Bash(jw done:*)"

    # Images attached to tasks by the capture tool.
    "Read(//run/user/*/agent-task/**)"

    # Read-only glab. Not `glab api` (can POST/PUT), `glab ci view` or
    # `glab issue board view` (interactive TUIs that can run/retry jobs).
    "Bash(glab mr list:*)"
    "Bash(glab mr view:*)"
    "Bash(glab mr diff:*)"
    "Bash(glab mr approvers:*)"
    "Bash(glab mr issues:*)"
    "Bash(glab mr note list:*)"
    "Bash(glab issue list:*)"
    "Bash(glab issue view:*)"
    "Bash(glab ci list:*)"
    "Bash(glab ci get:*)"
    "Bash(glab ci status:*)"
    "Bash(glab ci trace:*)"
    "Bash(glab ci lint:*)"
    "Bash(glab ci config compile:*)"
    "Bash(glab repo view:*)"
    "Bash(glab repo list:*)"
    "Bash(glab repo contributors:*)"
    "Bash(glab release list:*)"
    "Bash(glab release view:*)"
    "Bash(glab label list:*)"
    "Bash(glab label get:*)"

    "Bash(forge repo:*)"
    "Bash(forge pr list:*)"
    "Bash(forge pr view:*)"
    "Bash(forge pr checks:*)"
    "Bash(forge pr comments:*)"
    "Bash(forge pr drafts:*)"
    "Bash(forge pr review:*)"

    "Bash(tm:*)"

    "Bash(mvn test:*)"
    "Bash(mvn compile:*)"
    "Bash(mvn verify:*)"
  ];
  deny = [
    # Sessions are started only with `jw agent`: others have no jw session record.
    "Bash(shpool attach:*)"
    "Bash(shpool --no-daemonize attach:*)"

    "Edit(~/.local/state/agent-sessions/**)"

    # MR/PR comments, replies and resolves go only through `forge pr review` drafts.
    "Bash(glab mr note create:*)"
    "Bash(glab mr note update:*)"
    "Bash(glab mr note delete:*)"
    "Bash(glab mr note resolve:*)"
    "Bash(glab mr note reopen:*)"
    "Bash(glab mr note *-m *)"
    "Bash(glab mr note *--message*)"
    "Bash(glab mr note *--resolve*)"
    "Bash(glab mr note *--unresolve*)"
    "Bash(glab api *notes*)"
    "Bash(glab api *discussions*)"
    "Bash(glab api *mutation*)"
    "Bash(curl *notes*)"
    "Bash(curl *discussions*)"
    # The token would let any HTTP client bypass the rules above.
    "Bash(glab config get token*)"
    "Bash(glab auth status *-t*)"
    "Read(~/.config/glab-cli/**)"
    "Bash(gh pr comment:*)"
    "Bash(gh pr review:*)"
    "Bash(gh api *comments*)"
    "Bash(gh api *reviews*)"
  ];
  ask = ["Edit(@knowledgeFile@)"];
  additionalDirectories = ["@memoryDir@" "@projectsDir@"];
  # every session starts in auto mode, also those jw and the event handlers start
  defaultMode = "auto";
}
