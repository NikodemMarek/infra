{
  lib,
  python3Packages,
  git,
}:

python3Packages.buildPythonApplication {
  pname = "knowledge";
  version = "0.1.0";
  pyproject = true;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./pyproject.toml
      ./src
      ./tests
    ];
  };

  build-system = [ python3Packages.setuptools ];
  nativeBuildInputs = [ python3Packages.mypy ];

  # types are part of the build: a mypy --strict error fails it
  preBuild = ''
    MYPY_CACHE_DIR="$TMPDIR/mypy" mypy
  '';

  nativeCheckInputs = [
    python3Packages.pytestCheckHook
    git
  ];
  pythonImportsCheck = [ "knowledge" ];

  # git first: the memory repo must be handled by the git this was tested with
  makeWrapperArgs = [
    "--prefix"
    "PATH"
    ":"
    (lib.makeBinPath [ git ])
  ];

  meta = {
    description = "Knowledge curator: collects what agents learn and keeps a curated store in the memory repo";
    mainProgram = "knowledge";
  };
}
