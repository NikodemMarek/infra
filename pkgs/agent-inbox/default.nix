{
  lib,
  rustPlatform,
  makeWrapper,
  jw,
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

  # `N` in the sessions view runs `jw agent -d`; a jw of the user's own PATH wins. zellij and
  # shpool come from PATH too: they must match the running server and daemon.
  postInstall = ''
    wrapProgram $out/bin/agent-inbox --suffix PATH : ${lib.makeBinPath [jw]}
  '';

  meta = {
    description = "Terminal UI over the event router's queue and Claude's live sessions: inspect agent events, answer sudo password requests and Claude permission prompts and questions, attach to, kill and start agents";
    mainProgram = "agent-inbox";
  };
}
