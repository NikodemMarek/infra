# Claude Code permission rules (https://code.claude.com/docs/en/permissions),
# imported by settings.nix as `permissions`. `@memoryDir@` is substituted at
# build time.
{
  allow = [
    "Bash(jj log:*)"
    "Bash(jj status:*)"
    "Bash(jj show:*)"
    "Bash(jj diff:*)"
    "Bash(jj new:*)"
    "Bash(jj squash:*)"
    "Bash(jj split:*)"
    "Bash(jj edit:*)"
    "Bash(jj describe:*)"
    "Bash(jj commit:*)"
    "Bash(jj rebase:*)"
    "Bash(jj upload:*)"
    "Bash(jw agent -d:*)"
    "Bash(jw msg:*)"

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

    # Queueing notes for the knowledge curator.
    "Bash(knowledge submit:*)"

    "Bash(mvn test:*)"
    "Bash(mvn compile:*)"
  ];
  # The curator's files (injected into every session) and the note inbox: event
  # sessions run acceptEdits with the memory dir in additionalDirectories, so
  # without these they could write them directly. The architect's own files
  # (overview, decisions, log, notes) are not covered. Only Edit rules: they
  # apply to Write too, Write path rules are never consulted. Sessions are
  # started only with `jw agent`: others are invisible to agent-inbox.
  deny = [
    "Bash(shpool attach:*)"
    "Bash(shpool --no-daemonize attach:*)"
    "Edit(@memoryDir@/global.md)"
    "Edit(@memoryDir@/topic/**)"
    "Edit(@memoryDir@/lang/**)"
    "Edit(@memoryDir@/org/**)"
    "Edit(@memoryDir@/projects/*/facts.md)"
    "Edit(~/.local/state/knowledge/inbox/**)"

    # MR comments, replies and resolves go only through `gitlab-review` drafts.
    # Threads are read with `glab mr note list` (allowed), so all `glab api`
    # calls on notes/discussions can go.
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
  ];
  ask = [];
  additionalDirectories = ["@memoryDir@" "~/projects"];
  # every session starts in auto mode, also those jw and the event handlers start
  defaultMode = "auto";
}
