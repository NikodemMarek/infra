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
    ];
    # Covers AskUserQuestion too; `|| true` keeps a failing notification from ever blocking.
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
    ];
    # Event-started sessions only: with CLAUDE_EVENT_SESSION_DIR set it delivers queued event prompts (paths)
    # by blocking the stop; for KNOWLEDGE_ORIGIN sessions it otherwise posts claude.waiting (tells the user
    # the session waits).
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
    # Event-started sessions only (the hook checks CLAUDE_EVENT_SESSION_DIR): hands over queued follow-up prompts.
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
