# Rendered to $CLAUDE_CONFIG_DIR/settings.json (a read-only link into the
# store). In-session changes (/config, /theme, /plugin, ...) last only for the
# session; change this file or `claude.extend { settings = ...; }` to persist.
{
  permissions = import ./permissions.nix;

  statusLine = {
    type = "command";
    command = "claude-statusline";
  };

  # Built-in auto-memory is replaced by the knowledge notes and curator. Both the
  # setting and the env var, so a wrong key name cannot leave it on.
  autoMemoryEnabled = false;
  env = {
    CLAUDE_CODE_DISABLE_AUTO_MEMORY = "1";
  };

  hooks = {
    SessionStart = [
      {
        hooks = [
          {
            type = "command";
            command = "knowledge hook session-start";
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
    # Only sessions with a record (AGENT_SESSION: `jw agent`, events) reach the inbox and notifications;
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
    # SessionStart and the idle_prompt notification, ended on SessionEnd.
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
    SessionEnd = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-status-hook ended || true";
            timeout = 10;
          }
        ];
      }
    ];
    # With CLAUDE_EVENT_SESSION_DIR set it delivers queued event prompts (paths) by blocking the stop; for sessions with a record (AGENT_SESSION) it otherwise sets the status to idle and posts claude.waiting (tells the user
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
    # Hands over queued follow-up prompts (event-started sessions, CLAUDE_EVENT_SESSION_DIR) as context.
    UserPromptSubmit = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-prompt-hook || true";
            timeout = 10;
          }
        ];
      }
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
            command = "claude-jj-workspace create";
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
            command = "claude-jj-workspace remove";
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
            command = "claude-jj-workspace stop";
            timeout = 180;
          }
        ];
      }
      {
        hooks = [
          {
            type = "command";
            command = "knowledge hook subagent-stop";
            timeout = 30;
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
}
