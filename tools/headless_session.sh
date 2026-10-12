#!/bin/bash
# Start a private headless GNOME compositor (mutter --headless) with its own
# D-Bus session bus and run the app in it, maximized, with F12 screenshots.
# The user's desktop session is not touched. Input can then be injected
# through mutter's own org.gnome.Mutter.RemoteDesktop API on the private bus
# (see tools/live_input.py).
#
# usage: tools/headless_session.sh <state-dir> <WxH> [app args...]
# Writes <state-dir>/bus (DBUS_SESSION_BUS_ADDRESS) and <state-dir>/wayland.
set -u
STATE=$1; SIZE=$2; shift 2
mkdir -p "$STATE"
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
SOCK=f420-live-$$
exec dbus-run-session -- bash -c '
  echo "$DBUS_SESSION_BUS_ADDRESS" > "'"$STATE"'/bus"
  echo "'"$SOCK"'" > "'"$STATE"'/wayland"
  mutter --headless --wayland --no-x11 --virtual-monitor '"$SIZE"' --wayland-display '"$SOCK"' > "'"$STATE"'/mutter.log" 2>&1 &
  MP=$!
  trap "kill $MP 2>/dev/null" EXIT INT TERM HUP
  for i in $(seq 1 100); do [ -S "$XDG_RUNTIME_DIR/'"$SOCK"'" ] && break; sleep 0.1; done
  WAYLAND_DISPLAY='"$SOCK"' DISPLAY= "$@" > "'"$STATE"'/app.log" 2>&1
  RC=$?
  kill $MP 2>/dev/null; wait $MP 2>/dev/null
  echo $RC > "'"$STATE"'/exit"
' _ "$@"
