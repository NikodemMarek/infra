{
  lib,
  python3Packages,
  linkFarm,
  # event type -> package whose main program handles it. An event goes to its
  # type, else the nearest prefix ("gitlab.note" -> "gitlab"), else `default`.
  handlers ? { },
  # event type prefixes ("claude" matches "claude" and "claude.question") whose
  # events are also queued in the in-memory inbox (GET /inbox, DELETE /inbox/<id>)
  inbox ? [ ],
  port ? 7531,
}:

let
  handlersDir = linkFarm "event-router-handlers" (
    lib.mapAttrsToList (name: drv: {
      inherit name;
      path = lib.getExe drv;
    }) handlers
  );
  url = "http://127.0.0.1:${toString port}";
in
python3Packages.buildPythonApplication {
  pname = "event-router";
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
  pythonImportsCheck = [ "event_router" ];

  makeWrapperArgs = [
    "--set"
    "EVENT_ROUTER_HANDLERS"
    "${handlersDir}"
    "--set"
    "EVENT_ROUTER_INBOX"
    (lib.concatStringsSep "," inbox)
    "--set"
    "EVENT_ROUTER_PORT"
    (toString port)
    "--set"
    "EVENT_ROUTER_URL"
    url
  ];

  passthru = { inherit handlers handlersDir inbox url; };

  meta = {
    description = "Receive events over HTTP and dispatch them to handlers";
    mainProgram = "event-router";
  };
}
