#!/usr/bin/python3
"""Inject real pointer/keyboard input into a PRIVATE headless mutter started by
tools/headless_session.sh, through mutter's own org.gnome.Mutter.RemoteDesktop
D-Bus API on that compositor's private session bus. Events travel through the
compositor's Wayland seat to the app exactly like physical input; nothing here
touches the user's desktop session.

Requires the system python (jeepney). Usage:

    /usr/bin/python3 tools/live_input.py <state-dir> <script-file|->

Script lines. Coordinates are app-window content coordinates (egui points, as
seen in F12 screenshots); F420_OFFSET="dx,dy" (default "4,28", measured for
eframe's client-side decorations on a maximized window in mutter 50) is added
to get monitor pixels.
    move X Y                      click X Y [left|right|middle]
    dclick X Y                    drag X1 Y1 X2 Y2 [button] [steps]
    press [button] / release [button]
    key SPEC   (e.g. Escape, Return, F12, ctrl+z, ctrl+shift+s, shift+r, l)
    type TEXT  (ASCII letters, digits, '.', '-', '/', '_', space)
    scroll X Y STEPS              sleep MS
    shot                          (F12: the app saves --shot-dir/shot_NNN.png)
    # comment
"""
import os
import sys
import time

from jeepney import DBusAddress, Properties, new_method_call
from jeepney.io.blocking import open_dbus_connection

OFFSET = tuple(float(v) for v in os.environ.get("F420_OFFSET", "4,28").split(","))
BUTTONS = {"left": 0x110, "right": 0x111, "middle": 0x112}
KEYSYMS = {
    "escape": 0xFF1B, "return": 0xFF0D, "enter": 0xFF0D, "tab": 0xFF09,
    "backspace": 0xFF08, "delete": 0xFFFF, "f1": 0xFFBE, "f9": 0xFFC6, "f12": 0xFFC9,
    "ctrl": 0xFFE3, "shift": 0xFFE1, "alt": 0xFFE9, "space": 0x20,
}


def keysym(name):
    n = name.lower()
    if n in KEYSYMS:
        return KEYSYMS[n]
    if len(name) == 1:
        return ord(name)
    raise ValueError(f"unknown key {name!r}")


class Driver:
    def __init__(self, bus_address):
        os.environ["DBUS_SESSION_BUS_ADDRESS"] = bus_address
        self.conn = open_dbus_connection(bus="SESSION")
        rd = DBusAddress("/org/gnome/Mutter/RemoteDesktop",
                         bus_name="org.gnome.Mutter.RemoteDesktop",
                         interface="org.gnome.Mutter.RemoteDesktop")
        path = self.call(new_method_call(rd, "CreateSession"))[0]
        self.session = DBusAddress(path, bus_name="org.gnome.Mutter.RemoteDesktop",
                                   interface="org.gnome.Mutter.RemoteDesktop.Session")
        sid = self.call(Properties(self.session).get("SessionId"))[0][1]
        sc = DBusAddress("/org/gnome/Mutter/ScreenCast",
                         bus_name="org.gnome.Mutter.ScreenCast",
                         interface="org.gnome.Mutter.ScreenCast")
        scpath = self.call(new_method_call(
            sc, "CreateSession", "a{sv}", ({"remote-desktop-session-id": ("s", sid)},)))[0]
        scs = DBusAddress(scpath, bus_name="org.gnome.Mutter.ScreenCast",
                          interface="org.gnome.Mutter.ScreenCast.Session")
        self.stream = self.call(new_method_call(
            scs, "RecordMonitor", "sa{sv}", (os.environ.get("F420_MONITOR", "Meta-0"), {})))[0]
        self.call(new_method_call(self.session, "Start"))
        time.sleep(0.3)
        # The first key event of a new session is dropped while mutter sets up
        # the virtual keyboard (observed); send a harmless Shift tap first.
        self.key(KEYSYMS["shift"], True)
        self.key(KEYSYMS["shift"], False)
        # Likewise the first pointer motion of a new virtual pointer (observed).
        for _ in range(2):
            self._s("NotifyPointerMotionAbsolute", "sdd", (self.stream, 1.0, 1.0))
            time.sleep(0.1)
        time.sleep(0.2)

    def call(self, msg):
        reply = self.conn.send_and_get_reply(msg, timeout=10)
        if reply.header.message_type.name == "error":
            raise RuntimeError(f"D-Bus error: {reply.body}")
        return reply.body

    def _s(self, method, sig=None, args=()):
        self.call(new_method_call(self.session, method, sig, args))

    def move(self, x, y):
        x, y = x + OFFSET[0], y + OFFSET[1]
        self._s("NotifyPointerMotionAbsolute", "sdd", (self.stream, float(x), float(y)))
        time.sleep(0.02)

    def button(self, b, down):
        self._s("NotifyPointerButton", "ib", (BUTTONS[b], down))
        time.sleep(0.03)

    def key(self, ks, down):
        self._s("NotifyKeyboardKeysym", "ub", (ks, down))
        time.sleep(0.02)

    def chord(self, spec):
        parts = spec.split("+") if len(spec) > 1 else [spec]
        syms = [keysym(p) for p in parts]
        for s in syms:
            self.key(s, True)
        for s in reversed(syms):
            self.key(s, False)
        time.sleep(0.05)

    def scroll(self, steps):
        self._s("NotifyPointerAxisDiscrete", "ui", (0, int(steps)))
        time.sleep(0.05)


def run(driver, lines):
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        op, *a = line.split(None, 1)
        args = a[0].split() if a else []
        if op == "move":
            driver.move(*map(float, args[:2]))
        elif op in ("click", "dclick"):
            x, y = map(float, args[:2])
            b = args[2] if len(args) > 2 else "left"
            driver.move(x, y)
            time.sleep(0.05)
            for _ in range(2 if op == "dclick" else 1):
                driver.button(b, True)
                driver.button(b, False)
            time.sleep(0.1)
        elif op == "drag":
            x1, y1, x2, y2 = map(float, args[:4])
            b = args[4] if len(args) > 4 else "left"
            steps = int(args[5]) if len(args) > 5 else 12
            driver.move(x1, y1)
            driver.button(b, True)
            for k in range(1, steps + 1):
                t = k / steps
                driver.move(x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)
            driver.button(b, False)
            time.sleep(0.1)
        elif op == "press":
            driver.button(args[0] if args else "left", True)
        elif op == "release":
            driver.button(args[0] if args else "left", False)
        elif op == "key":
            driver.chord(args[0])
        elif op == "type":
            for ch in a[0] if a else "":
                driver.chord("space" if ch == " " else ch)
        elif op == "scroll":
            driver.move(float(args[0]), float(args[1]))
            driver.scroll(int(args[2]))
        elif op == "sleep":
            time.sleep(float(args[0]) / 1000.0)
        elif op == "shot":
            driver.chord("F12")
            time.sleep(0.8)
        else:
            raise ValueError(f"unknown op {op!r}")
        print(f"ok {line}", flush=True)


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(64)
    state, script = sys.argv[1], sys.argv[2]
    with open(os.path.join(state, "bus")) as f:
        bus = f.read().strip()
    lines = sys.stdin.read().splitlines() if script == "-" else open(script).read().splitlines()
    run(Driver(bus), lines)


if __name__ == "__main__":
    main()
