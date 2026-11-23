# `jw`: the binary does the work; the fish function cds where it says.
# fzf, shpool, direnv, cksum, forge and event-pub are on a PATH suffix, so the user's own win; jj and claude come from
# the user's PATH.
{
  lib,
  rustPlatform,
  makeWrapper,
  jujutsu,
  git,
  fzf,
  shpool,
  direnv,
  coreutils,
  forge,
  event-bus,
  projectsRoot ? "~/projects",
}:
assert lib.assertMsg (builtins.match "[~/][A-Za-z0-9._/-]*" projectsRoot != null) "jw: projectsRoot must be ~/... or an absolute path of [A-Za-z0-9._/-]";
  rustPlatform.buildRustPackage {
    pname = "jw";
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
    nativeCheckInputs = [jujutsu git];

    postInstall = ''
      wrapProgram $out/bin/jw \
        --set JW_PROJECTS_ROOT '${projectsRoot}' \
        --suffix PATH : ${lib.makeBinPath [fzf shpool direnv coreutils forge event-bus]}
      install -Dm644 ${./jw.fish} $out/share/fish/vendor_functions.d/jw.fish
      install -Dm644 ${./completions.fish} $out/share/fish/vendor_completions.d/jw.fish
      substituteInPlace $out/share/fish/vendor_completions.d/jw.fish \
        --replace-fail '~/projects/*/.jj' '${projectsRoot}/*/.jj'
    '';

    meta = {
      description = "Quick jj workspaces for the repos under one root, and Claude sessions in them";
      mainProgram = "jw";
    };
  }
