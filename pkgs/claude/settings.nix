# Rendered to $CLAUDE_CONFIG_DIR/settings.json (a read-only link into the
# store). In-session changes (/config, /theme, /plugin, ...) last only for the
# session; change this file or `claude.extend { settings = ...; }` to persist.
{
  permissions = import ./permissions.nix;

  statusLine = {
    type = "command";
    command = "claude-statusline";
    refreshInterval = 30;
  };

  # Built-in auto-memory stays off (knowledge lives in AGENTS.md and the knowledge file);
  # both keys, so a wrong key name cannot leave it on.
  autoMemoryEnabled = false;
  disableClaudeAiConnectors = true;
  switchModelsOnFlag = true;
  env = {
    CLAUDE_CODE_DISABLE_AUTO_MEMORY = "1";
    CLAUDE_CODE_AUTO_COMPACT_WINDOW = "300000";
  };

  hooks = {
    SessionStart = [
      # After a compaction the summary can drop the stack and session state the coordinator works from.
      {
        matcher = "compact";
        hooks = [
          {
            type = "command";
            command = "jj log --no-graph -r 'trunk()..@' 2>/dev/null; [ -n \"$AGENT_SESSION\" ] && jw session get 2>/dev/null; true";
            timeout = 10;
          }
        ];
      }
      {
        matcher = "startup|resume|clear";
        hooks = [
          {
            type = "command";
            command = "claude-status-hook idle || true";
            timeout = 10;
          }
        ];
      }
    ];
    PreToolUse = [
      {
        matcher = "Bash";
        hooks = [
          {
            type = "command";
            command = "claude-sudo-guard";
            timeout = 10;
          }
        ];
      }
    ];
    # Covers AskUserQuestion too; `|| true` keeps a failing notification from ever blocking.
    # Only sessions with a record (AGENT_SESSION: `jw agent`) reach the inbox and notifications;
    # hand-started sessions get only the local dialog.
    # Waits up to CLAUDE_INBOX_WAIT seconds (default and max 600; 0 = notification only) for an answer
    # from agent-inbox while the local dialog is shown; the timeout covers that wait.
    PermissionRequest = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-permission-hook || true";
            timeout = 610;
          }
        ];
      }
      {
        hooks = [
          {
            type = "command";
            command = "claude-status-hook waiting || true";
            timeout = 10;
          }
        ];
      }
    ];
    # The session record status (jw session status) follows the hooks: busy on UserPromptSubmit and
    # PostToolUse/PostToolUseFailure (back to work after a permission prompt), waiting on PermissionRequest, idle on Stop,
    # SessionStart and the idle_prompt notification. Nothing marks a session's end: the status keeps its last activity.
    # Outside busy it also records the estimated prompt cache expiry.
    PostToolUse = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-status-hook busy || true";
            timeout = 10;
          }
        ];
      }
    ];
    PostToolUseFailure = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-status-hook busy || true";
            timeout = 10;
          }
        ];
      }
    ];
    Notification = [
      {
        matcher = "idle_prompt";
        hooks = [
          {
            type = "command";
            command = "claude-status-hook idle || true";
            timeout = 10;
          }
        ];
      }
    ];
    # For sessions with a record (AGENT_SESSION) it sets the status to idle and posts claude.waiting (tells the user
    # the session waits, titled with the first line of the last message).
    # Permission prompts and questions are already routed by PermissionRequest.
    Stop = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-stop-hook || true";
            timeout = 10;
          }
        ];
      }
    ];
    UserPromptSubmit = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-status-hook busy || true";
            timeout = 10;
          }
        ];
      }
    ];
    WorktreeCreate = [
      {
        hooks = [
          {
            type = "command";
            command = "jw hook create";
            timeout = 120;
          }
        ];
      }
    ];
    WorktreeRemove = [
      {
        hooks = [
          {
            type = "command";
            command = "jw hook remove";
            timeout = 180;
          }
        ];
      }
    ];
    SubagentStop = [
      {
        hooks = [
          {
            type = "command";
            command = "jw hook stop || true";
            timeout = 180;
          }
        ];
      }
    ];
  };

  enabledPlugins = {
    "rust-analyzer-lsp@claude-plugins-official" = true;
  };

  # No AI attribution in commits or PRs (see CLAUDE.md); both keys, as with auto-memory.
  attribution = {
    commit = "";
    pr = "";
  };
  includeCoAuthoredBy = false;

  effortLevel = "xhigh";
  theme = "dark";
  skipAutoPermissionPrompt = true;
  skipDangerousModePermissionPrompt = true;
  # Bypass-mode sessions otherwise hold `jw msg` messages (no sender mode attested) for manual approval.
  crossSessionInbound = "accept";
}
