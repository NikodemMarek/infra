{
  lib,
  pkgs,
  ...
}: let
  whisperModel = pkgs.fetchurl {
    url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q8_0.bin";
    sha256 = "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1";
  };
  # Set to "0.0.0.0" to serve other tailnet hosts (tailscale0 is trusted, see tailscale.nix).
  host = "127.0.0.1";
in {
  systemd.services.whisper-server = {
    description = "whisper.cpp speech-to-text server (dictate)";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    serviceConfig = {
      ExecStart = "${lib.getExe' pkgs.whisper-cpp-vulkan "whisper-server"} --host ${host} --port 8178 --model ${whisperModel} --language en";
      DynamicUser = true;
      Restart = "on-failure";
    };
  };

  environment.systemPackages = [pkgs.dictate];
}
