{
  lib,
  python3Packages,
  natscli,
}:
python3Packages.buildPythonApplication {
  pname = "matrix-gateway";
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
  pythonImportsCheck = ["matrix_gateway"];

  makeWrapperArgs = [
    "--prefix"
    "PATH"
    ":"
    (lib.makeBinPath [natscli])
  ];

  meta = {
    description = "Relays between NATS and a Matrix room: posts requests as the bot and publishes the owner's signed replies";
    mainProgram = "matrix-gateway";
  };
}
