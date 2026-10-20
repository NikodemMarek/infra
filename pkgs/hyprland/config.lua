-- Monitor configuration
hl.monitor({
  output = "",
  mode = "preferred",
  position = "auto",
  scale = 1,
})

-- General Configuration
hl.config({
  general = {
    border_size = 0,
    col = {
      active_border = "rgb(89b4fa)",
      inactive_border = "rgb(45475a)",
    },
    gaps_in = 5,
    gaps_out = 5,
    layout = "dwindle",
  },

  animations = {
    enabled = true,
  },

  decoration = {
    shadow = {
      color = "rgba(1e1e2e99)",
    },
    rounding = 8,
  },

  group = {
    groupbar = {
      col = {
        active = "rgb(89b4fa)",
        inactive = "rgb(45475a)",
      },
      text_color = "rgb(cdd6f4)",
    },
    col = {
      border_active = "rgb(89b4fa)",
      border_inactive = "rgb(45475a)",
      border_locked_active = "rgb(94e2d5)",
    },
  },

  input = {
    touchpad = {
      disable_while_typing = true,
      middle_button_emulation = true,
      natural_scroll = false,
      tap_and_drag = true,
      tap_to_click = true,
    },
    follow_mouse = 1,
    kb_layout = "pl",
    numlock_by_default = true,
    sensitivity = 0,
  },

  dwindle = {
    preserve_split = true,
  },

  master = {
    mfact = 0.750000,
    new_status = "master",
    orientation = "right",
  },

  misc = {
    background_color = "rgb(1e1e2e)",
    disable_hyprland_logo = true,
    disable_splash_rendering = true,
  },
})

hl.gesture({
  fingers = 3,
  direction = "vertical",
  action = "workspace"
})

-- Curves
hl.curve("bezierWindow",    { type = "bezier", points = { {0.05, 0.9}, {0.1, 1.05} } })
hl.curve("bezierWindowOut", { type = "bezier", points = { {0.05, 0.9}, {0.1, 1.05} } })
hl.curve("bezierWindowIn",  { type = "bezier", points = { {0.05, 0.9}, {0.1, 1.05} } })

-- Animations
hl.animation({ leaf = "windows",    enabled = true, speed = 7, bezier = "bezierWindow" })
hl.animation({ leaf = "windowsOut", enabled = true, speed = 7, bezier = "bezierWindowOut", style = "slide" })
hl.animation({ leaf = "windowsIn",  enabled = true, speed = 7, bezier = "bezierWindowIn",  style = "slide" })
hl.animation({ leaf = "border",     enabled = true, speed = 10, bezier = "default" })
hl.animation({ leaf = "fade",       enabled = true, speed = 7, bezier = "default" })
hl.animation({ leaf = "workspaces", enabled = true, speed = 6, bezier = "default",        style = "slidevert" })

local mainMod = "SUPER"

-- Focus navigation
hl.bind(mainMod .. " + H", hl.dsp.focus({ direction = "left"}))
hl.bind(mainMod .. " + L", hl.dsp.focus({ direction = "right"}))
hl.bind(mainMod .. " + J", hl.dsp.focus({ direction = "down"}))
hl.bind(mainMod .. " + K", hl.dsp.focus({ direction = "up"}))

-- Workspace navigation
hl.bind(mainMod .. " + TAB", hl.dsp.focus({ workspace  = "m+1" }))
hl.bind(mainMod .. " + SHIFT + TAB", hl.dsp.focus({ workspace  = "m-1" }))
hl.bind(mainMod .. " + CTRL + TAB", hl.dsp.window.move({ workspace  = "m+1" }))
hl.bind(mainMod .. " + CTRL + SHIFT + TAB", hl.dsp.window.move({ workspace  = "m-1" }))
hl.bind(mainMod .. " + CTRL + N", hl.dsp.window.move({ workspace  = "emptynm" }))

hl.bind(mainMod .. " + W", hl.dsp.window.close())

hl.bind(mainMod .. " + T", hl.dsp.window.fullscreen({ action = "toggle" }))
hl.bind(mainMod .. " + B", hl.dsp.window.float({ action = "toggle" }))

-- Workspaces 1-10 Loop
for i = 1, 10 do
    local key = i % 10 -- 10 maps to key 0
    hl.bind(mainMod .. " + " .. key,             hl.dsp.focus({ workspace = i}))
    hl.bind(mainMod .. " + SHIFT + " .. key,     hl.dsp.window.move({ workspace = i }))
end
-- Mouse Binds
hl.bind(mainMod .. " + mouse:272", hl.dsp.window.drag(),   { mouse = true })
hl.bind(mainMod .. " + mouse:273", hl.dsp.window.resize(), { mouse = true })

-- Applications
hl.bind(mainMod .. " + RETURN", hl.dsp.exec_cmd("uwsm-app -s b -- alacritty"), { locked = false })
hl.bind(mainMod .. " + A", hl.dsp.exec_cmd("uwsm-app -s b -- zen"), { locked = false })
hl.bind(mainMod .. " + E", hl.dsp.exec_cmd("alacritty --command yazi"), { locked = false })
hl.bind(mainMod .. " + SPACE", hl.dsp.exec_cmd("rofi -show drun"), { locked = false })
hl.bind(mainMod .. " + C", hl.dsp.exec_cmd("agent-task"), { locked = false })
hl.bind(mainMod .. " + SHIFT + C", hl.dsp.exec_cmd("agent-task --shot"), { locked = false })

-- Screenshots & Session
hl.bind("PRINT", hl.dsp.exec_cmd("grim -g '$(slurp)' - | wl-copy"), { locked = false })
hl.bind("SHIFT + PRINT", hl.dsp.exec_cmd("grim -g '$(slurp)' ~/screenshots/$(date +'%Y-%m-%d_%H:%M:%S').png"), { locked = false })
hl.bind(mainMod .. " + SHIFT + ESCAPE", hl.dsp.exec_cmd("loginctl lock-session"), { locked = false })
hl.bind(mainMod .. " + ALT + ESCAPE", hl.dsp.exit(), { locked = true })

-- Media Keys
hl.bind("XF86AudioRaiseVolume", hl.dsp.exec_cmd("wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 5%+"), { locked = true, repeating = true })
hl.bind("XF86AudioLowerVolume", hl.dsp.exec_cmd("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"),      { locked = true, repeating = true })
hl.bind("XF86AudioMute",        hl.dsp.exec_cmd("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),     { locked = true, repeating = true })
hl.bind("XF86AudioMicMute",     hl.dsp.exec_cmd("wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"),   { locked = true, repeating = true })
hl.bind("XF86MonBrightnessUp",  hl.dsp.exec_cmd("brightnessctl -e4 -n2 set 5%+"),                  { locked = true, repeating = true })
hl.bind("XF86MonBrightnessDown",hl.dsp.exec_cmd("brightnessctl -e4 -n2 set 5%-"),                  { locked = true, repeating = true })

-- Startup Applications
hl.on("hyprland.start", function ()
  hl.exec_cmd("uwsm-app -s b -- kanshi")
  hl.exec_cmd("uwsm-app -s b -- hypridle")
  hl.exec_cmd("uwsm-app -s b -- hyprpaper")
  hl.exec_cmd("uwsm-app -s b -- dunst")
  hl.exec_cmd("uwsm-app -s b -- waybar")
end)
