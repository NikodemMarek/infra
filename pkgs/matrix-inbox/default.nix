{
  lib,
  python3Packages,
  event-handlers,
  natscli,
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

  build-system = [python3Packages.setuptools];
  dependencies = [python3Packages.cryptography];
  nativeBuildInputs = [python3Packages.mypy];

  # types are part of the build: a mypy --strict error fails it
  preBuild = ''
    MYPY_CACHE_DIR="$TMPDIR/mypy" mypy
  '';

  nativeCheckInputs = [python3Packages.pytestCheckHook];
  pythonImportsCheck = ["matrix_inbox"];

  makeWrapperArgs = [
    "--prefix"
    "PATH"
    ":"
    (lib.makeBinPath [
      event-handlers.task
      natscli
    ])
    "--set-default"
    "MATRIX_GATEWAY_KEY"
    (lib.fileContents ./gateway.pub)
  ];

  meta = {
    description = "Runs the matrix-gateway's signed commands";
    mainProgram = "matrix-inbox-command";
  };
}
