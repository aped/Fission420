#!/bin/bash
# Start a private headless GNOME compositor (mutter --headless) with its own
# D-Bus session bus and run the app in it, maximized, with F12 screenshots.
# The user's desktop session is not touched. Input can then be injected
# through mutter's own org.gnome.Mutter.RemoteDesktop API on the private bus
# (see tools/live_input.py).
#
# usage: tools/headless_session.sh <state-dir> <WxH> [app args...]
# Writes <state-dir>/bus (DBUS_SESSION_BUS_ADDRESS), <state-dir>/wayland and
# <state-dir>/unit.
#
# Containment (2026-10-12): the whole session -- dbus-run-session, its private
# dbus-daemon, mutter, anything the private bus activates (gvfsd), and the app -- runs
# in its own transient systemd user scope (cgroup) named in <state-dir>/unit. Stopping
# that unit (`systemctl --user stop "$(cat <state-dir>/unit)"`, or
# tools/headless_cleanup.sh) kills all of it even when the wrapper itself was killed;
# RuntimeMaxSec bounds a forgotten session. Before this, killing the wrapper orphaned
# its children: runs 2-4 left 74 mutters, 192 private dbus-daemons and 192 gvfsd behind.
# gvfs is disabled inside the session (GIO_USE_VFS=local): the private bus otherwise
# activates a gvfsd that outlives it.
#
# Renderer (2026-10-12): the app runs with WGPU_BACKEND=${RCAD_WGPU_BACKEND:-gl}. With
# wgpu's default Vulkan backend (Mesa 26.2 RADV, FIFO present) under headless mutter 50.5,
# the app intermittently stops drawing: its UI thread idles in ep_poll with a redraw
# requested, i.e. the Wayland frame callback for its last commit never arrives (winit only
# delivers a requested redraw after it). The "hole" scene stalled in 3/6, 5/6 (low-power
# adapter) and 3/4 attempts on Vulkan, and 0/18 with the GL backend. Set
# RCAD_WGPU_BACKEND=vulkan to reproduce.
set -u
STATE=$1; SIZE=$2; shift 2
mkdir -p "$STATE"
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
SOCK=rcad-live-$$
UNIT=rcad-headless-$$-$(date +%s)
echo "$UNIT.scope" > "$STATE/unit"
exec systemd-run --user --scope --quiet --collect --unit="$UNIT" \
  -p RuntimeMaxSec="${RCAD_SESSION_MAX_SEC:-1800}" \
  env GIO_USE_VFS=local GVFS_DISABLE_FUSE=1 \
  dbus-run-session -- bash -c '
  echo "$DBUS_SESSION_BUS_ADDRESS" > "'"$STATE"'/bus"
  echo "'"$SOCK"'" > "'"$STATE"'/wayland"
  mutter --headless --wayland --no-x11 --virtual-monitor '"$SIZE"' --wayland-display '"$SOCK"' > "'"$STATE"'/mutter.log" 2>&1 &
  MP=$!
  trap "kill $MP 2>/dev/null" EXIT INT TERM HUP
  for i in $(seq 1 100); do [ -S "$XDG_RUNTIME_DIR/'"$SOCK"'" ] && break; sleep 0.1; done
  # The socket appears before the virtual monitor exists; a window created with no
  # output can wait forever for its first frame callback. Wait for the monitor.
  for i in $(seq 1 100); do grep -q "Added virtual monitor" "'"$STATE"'/mutter.log" 2>/dev/null && break; sleep 0.1; done
  WAYLAND_DISPLAY='"$SOCK"' DISPLAY= WGPU_BACKEND='"${RCAD_WGPU_BACKEND:-gl}"' "$@" > "'"$STATE"'/app.log" 2>&1
  RC=$?
  kill $MP 2>/dev/null; wait $MP 2>/dev/null
  echo $RC > "'"$STATE"'/exit"
' _ "$@"
