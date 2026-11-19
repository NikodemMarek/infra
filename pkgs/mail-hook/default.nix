{
  lib,
  python3Packages,
  natscli,
}:
python3Packages.buildPythonApplication {
  pname = "mail-hook";
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
  pythonImportsCheck = ["mail_hook"];

  makeWrapperArgs = [
    "--prefix"
    "PATH"
    ":"
    (lib.makeBinPath [natscli])
  ];

  meta = {
    description = "HTTP ingress that publishes received mail as mail.received events to NATS";
    mainProgram = "mail-hook";
  };
}
