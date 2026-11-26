# `plan-review`: event-bus, libnotify and xdg-utils are on a PATH suffix, so the user's own win.
{
  lib,
  rustPlatform,
  makeWrapper,
  event-bus,
  libnotify,
  xdg-utils,
}:
rustPlatform.buildRustPackage {
  pname = "plan-review";
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

  nativeBuildInputs = [makeWrapper];

  postInstall = ''
    wrapProgram $out/bin/plan-review \
      --suffix PATH : ${lib.makeBinPath [event-bus libnotify xdg-utils]}
  '';

  meta = {
    description = "Show an HTML plan in the browser and send the user's feedback back to the agent session";
    mainProgram = "plan-review";
  };
}
