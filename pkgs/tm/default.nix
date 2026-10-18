{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "tm";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      ./tests
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "CLI for the markdown task lists (raw, backlog, ISO-week files)";
    mainProgram = "tm";
  };
}
