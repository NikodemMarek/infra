{
  lib,
  python3Packages,
  bitwarden-cli,
  event-bus,
  secretspec,
}:
python3Packages.buildPythonApplication {
  pname = "secret-broker";
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
  nativeBuildInputs = [python3Packages.mypy];

  # types are part of the build: a mypy --strict error fails it
  preBuild = ''
    MYPY_CACHE_DIR="$TMPDIR/mypy" mypy
  '';

  nativeCheckInputs = [python3Packages.pytestCheckHook];
  pythonImportsCheck = ["secret_broker"];

  makeWrapperArgs = [
    "--set"
    "SECRET_BROKER_BW"
    "${bitwarden-cli}/bin/bw"
    "--set"
    "SECRET_BROKER_SECRETSPEC"
    "${secretspec}/bin/secretspec"
    "--set"
    "SECRET_BROKER_EVENT_PUB"
    "${event-bus}/bin/event-pub"
  ];

  meta = {
    description = "Run named actions with Bitwarden secrets after approval on a private console";
    mainProgram = "secret-broker";
  };
}
