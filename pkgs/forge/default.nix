{
  lib,
  rustPlatform,
  makeWrapper,
  jujutsu,
  git,
  jq,
}:
rustPlatform.buildRustPackage {
  pname = "forge";
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
  nativeCheckInputs = [
    jujutsu
    git
    jq
  ];

  postInstall = ''
    wrapProgram $out/bin/forge \
      --suffix PATH : ${lib.makeBinPath [jujutsu]}
  '';

  meta = {
    description = "Read and manage merge/pull requests on GitLab and GitHub through glab / gh";
    mainProgram = "forge";
    platforms = lib.platforms.unix;
  };
}
