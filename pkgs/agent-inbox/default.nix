{
  lib,
  rustPlatform,
  makeWrapper,
  jw,
  xdg-utils,
  event-bus,
  # Filter tabs, Tab cycles through them: a list of `{ name; kinds ? null; }`. `kinds` are the
  # session record kinds the tab shows (`review`, `note`, `task`, ...); null or absent = every kind no
  # other tab lists. Read from $AGENT_INBOX_TABS (JSON), which a value in the environment overrides.
  tabs ? [ ],
}:

rustPlatform.buildRustPackage {
  pname = "agent-inbox";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [makeWrapper];

  # `N` asks for a task and publishes it as `task.new` with event-pub; jw starts and attaches agents; `o` opens a link with xdg-open. A jw,
  # event-pub or xdg-open of the user's own PATH wins. zellij and shpool come from PATH too: they must match the
  # running server and daemon.
  postInstall = ''
    wrapProgram $out/bin/agent-inbox --suffix PATH : ${lib.makeBinPath [jw xdg-utils event-bus]} ${lib.optionalString (tabs != [ ]) "--set-default AGENT_INBOX_TABS ${lib.escapeShellArg (builtins.toJSON tabs)}"}
  '';

  meta = {
    description = "Terminal UI over the event inbox and Claude's live sessions: inspect agent events, answer sudo password requests and Claude permission prompts and questions, attach to, kill and start agents";
    mainProgram = "agent-inbox";
  };
}
