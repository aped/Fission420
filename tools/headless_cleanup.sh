#!/usr/bin/env bash
# Find (and with --kill, stop) processes left behind by the private headless GNOME
# sessions of tools/headless_session.sh:
#   * transient systemd user scopes rcad-headless-* (current harness; stopping one kills
#     everything in its cgroup);
#   * legacy orphans from harness versions before 2026-10-12, which ran without a scope:
#     - `mutter --headless ... --wayland-display rcad-live-*`;
#     - the private session bus `dbus-daemon --nofork --print-address N --session` started
#       by dbus-run-session, once orphaned (parent = this user's systemd --user);
#     - `gvfsd` (and its children) activated on such a private bus (its
#       DBUS_SESSION_BUS_ADDRESS names that daemon's /tmp/dbus-* socket).
# Never touches the user's real session bus (dbus-broker, /run/user/UID/bus), flatpak
# buses, or any process attached to them.
# usage: tools/headless_cleanup.sh [--kill]
set -u
KILL=0
[ "${1:-}" = "--kill" ] && KILL=1
UID_N=$(id -u)
SYSTEMD_USER=$(pgrep -u "$UID_N" -x systemd | head -1)
targets=()
note() { printf '%-8s %7s  %s\n' "$1" "$2" "$3"; }

# 1. Scopes of the current harness.
for unit in $(systemctl --user list-units --type=scope --all --plain --no-legend 'rcad-headless-*' 2>/dev/null | awk '{print $1}'); do
  note scope - "$unit"
  [ $KILL = 1 ] && systemctl --user stop "$unit"
done

# 2. Legacy headless mutters.
for p in $(pgrep -u "$UID_N" -f -- '^mutter --headless .*--wayland-display rcad-live-'); do
  note mutter "$p" "$(tr '\0' ' ' < /proc/$p/cmdline | cut -c1-100)"
  targets+=("$p")
done

# 3. Orphaned private session buses, and the sockets they listen on.
declare -A ours
for p in $(pgrep -u "$UID_N" -x dbus-daemon); do
  cmd=$(tr '\0' ' ' < /proc/$p/cmdline)
  [[ "$cmd" =~ ^dbus-daemon\ --nofork\ --print-address\ [0-9]+\ --session\ $ ]] || continue
  [ "$(ps -o ppid= -p "$p" | tr -d ' ')" = "$SYSTEMD_USER" ] || continue
  for s in $(ss -xlpn 2>/dev/null | grep "pid=$p," | grep -o '/tmp/dbus-[A-Za-z0-9]*'); do ours[$s]=1; done
  note dbus "$p" "orphaned private session bus"
  targets+=("$p")
done

# 4. gvfsd attached to one of those buses (plus its children).
for p in $(pgrep -u "$UID_N" -x gvfsd); do
  a=$(tr '\0' '\n' < /proc/$p/environ 2>/dev/null | grep '^DBUS_SESSION_BUS_ADDRESS=' | grep -o '/tmp/dbus-[A-Za-z0-9]*')
  [ -n "$a" ] || continue
  if [ -z "${ours[$a]:-}" ]; then
    # Also a private-bus gvfsd whose bus no longer exists at all (its daemon is
    # gone); a live bus that is not ours (e.g. an app's own) is left alone.
    ss -xlpn 2>/dev/null | grep -q "$a " && continue
  fi
  note gvfsd "$p" "bus $a"
  targets+=("$p")
  for c in $(pgrep -P "$p"); do note child "$c" "$(cat /proc/$c/comm)"; targets+=("$c"); done
done

echo "${#targets[@]} legacy process(es)"
if [ $KILL = 1 ] && [ ${#targets[@]} -gt 0 ]; then
  kill -TERM "${targets[@]}" 2>/dev/null
  sleep 2
  for p in "${targets[@]}"; do kill -0 "$p" 2>/dev/null && kill -KILL "$p" 2>/dev/null; done
  echo "stopped"
fi
