{
  stdenv,
  fetchFromGitHub,
  kernel,
  kernelModuleMakeFlags,
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

  makeFlags =
    kernelModuleMakeFlags
    ++ [
      "-C"
      "${kernel.dev}/lib/modules/${kernel.modDirVersion}/build"
      "M=$(PWD)/src"
      "INSTALL_MOD_PATH=${placeholder "out"}"
      "INSTALL_MOD_DIR=kernel/drivers/net/ethernet/realtek"
      "INSTALL_MOD_STRIP=1"
      "DEPMOD=true"
    ];

  buildFlags = ["modules"];

  installTargets = ["modules_install"];
}
