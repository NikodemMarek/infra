{
  lib,
  rustPlatform,
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

  meta = {
    description = "Terminal UI over the event router's queue: inspect agent events, answer sudo password requests and Claude permission prompts and questions";
    mainProgram = "agent-inbox";
  };
}
