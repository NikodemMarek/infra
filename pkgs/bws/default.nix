{
  stdenvNoCC,
  fetchzip,
}: let
  version = "2.1.0";
  arch = stdenvNoCC.hostPlatform.parsed.cpu.name;
  hashes = {
    x86_64 = "sha256-p79b+8ugF9yqCq9XRC9mOHI5wPUWrJ6lFa20pi8bW4M=";
    aarch64 = "sha256-/OpgX+kY/71NXcFyATX3n4w9mjxk3P92fhkIJZPj+P0=";
  };
in
  # the release binary: nixpkgs' bws is unfree, so it is not cached and builds from source
  stdenvNoCC.mkDerivation {
    pname = "bws";
    inherit version;
    src = fetchzip {
      url = "https://github.com/bitwarden/sdk-sm/releases/download/bws-v${version}/bws-${arch}-unknown-linux-musl-${version}.zip";
      hash = hashes.${arch};
      stripRoot = false;
    };
    installPhase = "install -Dm755 bws $out/bin/bws";
  }
