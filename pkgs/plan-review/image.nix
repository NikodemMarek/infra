{
  lib,
  dockerTools,
  plan-review,
}:
dockerTools.streamLayeredImage {
  name = "plan-review";
  config = {
    Entrypoint = [(lib.getExe plan-review) "serve"];
    Env = ["HOME=/state" "XDG_CONFIG_HOME=/state" "XDG_STATE_HOME=/state"];
  };
}
