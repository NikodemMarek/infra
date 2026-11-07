{
  lib,
  python3Packages,
  agent-inbox,
}:

python3Packages.buildPythonApplication {
  pname = "matrix-inbox";
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

  nativeCheckInputs = [ python3Packages.pytestCheckHook ];
  pythonImportsCheck = [ "matrix_inbox" ];

  makeWrapperArgs = [
    "--prefix"
    "PATH"
    ":"
    (lib.makeBinPath [ agent-inbox ])
  ];

  meta = {
    description = "Mirrors answerable agent-inbox items into a Matrix room and answers them from the owner's replies";
    mainProgram = "matrix-inbox";
  };
}
