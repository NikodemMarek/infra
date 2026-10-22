{
  lib,
  stdenvNoCC,
  python3,
  makeWrapper,
  jujutsu,
}:

# Used through the jj aliases `upload` (`jj util exec -- jj-upload`) and `sync`
# (`jj util exec -- jj-upload sync`). glab / gh are not bundled; they are the
# user's own and are looked up on PATH.
stdenvNoCC.mkDerivation {
  pname = "jj-upload";
  version = "0.1.0";

  dontUnpack = true;
  nativeBuildInputs = [ makeWrapper ];

  doCheck = true;
  checkPhase = ''
    runHook preCheck
    cp ${./jj-upload.py} jj_upload.py
    cp ${./test_jj_upload.py} test_jj_upload.py
    ${python3}/bin/python3 -m unittest
    runHook postCheck
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 ${./jj-upload.py} $out/bin/jj-upload
    substituteInPlace $out/bin/jj-upload \
      --replace-fail "#!/usr/bin/env python3" "#!${python3}/bin/python3"
    wrapProgram $out/bin/jj-upload \
      --suffix PATH : ${lib.makeBinPath [ jujutsu ]}
    runHook postInstall
  '';

  meta = {
    description = "Push a stack of jj changes as stacked merge/pull requests (GitLab, GitHub)";
    mainProgram = "jj-upload";
    platforms = lib.platforms.unix;
  };
}
