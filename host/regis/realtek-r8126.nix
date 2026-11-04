{
  stdenv,
  fetchFromGitHub,
  kernel,
}:
stdenv.mkDerivation {
  pname = "realtek-r8126";
  version = "unstable-2026-07-28";

  src = fetchFromGitHub {
    owner = "awesometic";
    repo = "realtek-r8126-dkms";
    rev = "fbf668194052211599594daa3d722c5c519d4c25";
    sha256 = "sha256-38m8Kj7BJ4kFPI82gV7XPAl05iAqDG0Z/BinSuu9Na8=";
  };

  nativeBuildInputs = kernel.moduleBuildDependencies;

  # Build directly in the src subdirectory where the kernel source files live
  buildPhase = ''
    runHook preBuild
    make -C ${kernel.dev}/lib/modules/${kernel.modDirVersion}/build M=$(pwd)/src modules
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/modules/${kernel.modDirVersion}/kernel/drivers/net/ethernet/realtek/

    # The compiled module lives inside src/
    cp src/r8126.ko $out/lib/modules/${kernel.modDirVersion}/kernel/drivers/net/ethernet/realtek/

    runHook postInstall
  '';
}
