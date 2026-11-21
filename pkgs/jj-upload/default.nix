{
  lib,
  rustPlatform,
  makeWrapper,
  jujutsu,
  git,
  jq,
  jw,
  forge,
}:
# Used through the jj aliases `upload` (`jj util exec -- jj-upload`), `sync`
# (`jj util exec -- jj-upload sync`) and `stack` (`jj util exec -- jj-upload stack`).
rustPlatform.buildRustPackage {
  pname = "jj-upload";
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
    wrapProgram $out/bin/jj-upload \
      --suffix PATH : ${lib.makeBinPath [jujutsu jw forge]}
  '';

  meta = {
    description = "Push a stack of jj changes as stacked merge/pull requests (GitLab, GitHub)";
    mainProgram = "jj-upload";
    platforms = lib.platforms.unix;
  };
}
