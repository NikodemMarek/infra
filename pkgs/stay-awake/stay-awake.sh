# Usage: stay-awake [on|off|toggle|status]  (default: status)
unit=stay-awake

active() { systemctl --user is-active --quiet "$unit"; }

on() {
  active && return
  systemd-run --user --quiet --unit="$unit" --collect \
    systemd-inhibit --what=sleep:idle:handle-lid-switch --mode=block \
    --who=stay-awake --why="Keeping the machine awake" sleep infinity
  notify-send -a stay-awake "Stay awake: on" "No suspend on idle or lid close" || true
}

off() {
  active || return 0
  systemctl --user stop "$unit"
  notify-send -a stay-awake "Stay awake: off" || true
}

case "${1:-status}" in
  on) on ;;
  off) off ;;
  toggle) if active; then off; else on; fi ;;
  status) if active; then echo on; else echo off; exit 1; fi ;;
  *) echo "usage: stay-awake [on|off|toggle|status]" >&2; exit 2 ;;
esac
