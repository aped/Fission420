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
    sclick X Y / smove X Y / sdrag X1 Y1 X2 Y2 [button]   sketch-plane mm
    wclick X Y Z                  world mm
    clickw LABEL                  click the widget the app registered as LABEL
    typew LABEL | TEXT            click a text field by label, select all, type
    clickdim NAME                 click the dimension label NAME (sketch or selected sketch)
    wait_idle [MS]                until the probe reports no work in flight / measuring
    expect PATH VALUE [TOL]       assert probe "doc" field (dotted path), e.g.
                                  expect volume 39214.6018 0.01 / expect editing.status FullyConstrained
      (s*/w* commands need the app started with F420_PROBE=<state>/probe.json;
       they project with the app's own camera, see crates/f420-app/src/main.rs)
    # comment
"""
import json
import math
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
    "plus": 0x2B, "equal": 0x3D, "minus": 0x2D,
}


def keysym(name):
    n = name.lower()
    if n in KEYSYMS:
        return KEYSYMS[n]
    if n.startswith("f") and n[1:].isdigit() and 1 <= int(n[1:]) <= 12:
        return 0xFFBE + int(n[1:]) - 1
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


class Probe:
    """Mirror of f420_viewport::Camera::project (Z-up orbit camera)."""

    def __init__(self, path):
        self.path = path
        self.failures = []

    def widget(self, key, timeout=3.0):
        deadline = time.time() + timeout
        while True:
            w = self.load().get("widgets", {})
            if key in w:
                x0, y0, x1, y1 = w[key]
                return ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
            if time.time() > deadline:
                raise RuntimeError(f"widget {key!r} not on screen; have {sorted(w)[:40]}")
            time.sleep(0.05)

    def doc_value(self, path):
        v = self.load().get("doc")
        for part in path.split("."):
            if not isinstance(v, dict) or part not in v:
                return None
            v = v[part]
        return v

    def load(self):
        for _ in range(20):
            try:
                with open(self.path) as f:
                    return json.load(f)
            except (OSError, ValueError):
                time.sleep(0.05)
        raise RuntimeError(f"no probe at {self.path}")

    def world(self, p):
        d = self.load()
        c, (x0, y0, x1, y1) = d["camera"], d["rect"]
        w, h = x1 - x0, y1 - y0
        sp, cp = math.sin(c["pitch"]), math.cos(c["pitch"])
        sy, cy = math.sin(c["yaw"]), math.cos(c["yaw"])
        back = (cp * cy, cp * sy, sp)
        eye = [c["target"][i] + back[i] * c["distance"] for i in range(3)]
        fwd = [-b for b in back]
        r = (fwd[1] * 1 - fwd[2] * 0, fwd[2] * 0 - fwd[0] * 1, 0.0)
        n = math.hypot(r[0], r[1]) or 1.0
        r = (r[0] / n, r[1] / n, 0.0)
        u = (r[1] * fwd[2] - r[2] * fwd[1], r[2] * fwd[0] - r[0] * fwd[2], r[0] * fwd[1] - r[1] * fwd[0])
        rel = [p[i] - eye[i] for i in range(3)]
        dot = lambda a, b: sum(a[i] * b[i] for i in range(3))
        z = dot(rel, fwd)
        t = math.tan(0.5 * c["fov_y"])
        nx = dot(rel, r) / (z * t * (w / h))
        ny = dot(rel, u) / (z * t)
        return (x0 + (nx + 1) * 0.5 * w, y0 + (1 - ny) * 0.5 * h)

    def sketch(self, x, y):
        s = self.load()["sketch"]
        if s is None:
            raise RuntimeError("no sketch session open")
        p = [s["origin"][i] + x * s["x"][i] + y * s["y"][i] for i in range(3)]
        return self.world(p)


def run(driver, lines, probe=None):
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
        elif op in ("sclick", "smove", "wclick"):
            v = list(map(float, args))
            sx, sy = probe.world(v[:3]) if op == "wclick" else probe.sketch(v[0], v[1])
            driver.move(sx, sy)
            if op != "smove":
                time.sleep(0.05)
                driver.button("left", True)
                driver.button("left", False)
                time.sleep(0.1)
        elif op == "sdrag":
            x1, y1, x2, y2 = map(float, args[:4])
            b = args[4] if len(args) > 4 else "left"
            (ax, ay), (bx, by) = probe.sketch(x1, y1), probe.sketch(x2, y2)
            driver.move(ax, ay)
            driver.button(b, True)
            for k in range(1, 13):
                t = k / 12
                driver.move(ax + (bx - ax) * t, ay + (by - ay) * t)
            driver.button(b, False)
            time.sleep(0.1)
        elif op in ("clickw", "clickdim"):
            key = a[0].strip() if a else ""
            if op == "clickdim":
                key = "dim:" + key
            x, y = probe.widget(key)
            driver.move(x, y)
            time.sleep(0.05)
            driver.button("left", True)
            driver.button("left", False)
            time.sleep(0.15)
        elif op == "typew":
            label, text = (a[0].split("|", 1) + [""])[:2]
            x, y = probe.widget(label.strip())
            driver.move(x, y)
            time.sleep(0.05)
            driver.button("left", True)
            driver.button("left", False)
            time.sleep(0.1)
            driver.chord("ctrl+a")
            for ch in text.strip():
                driver.chord("space" if ch == " " else ch)
            time.sleep(0.1)
        elif op == "wait_idle":
            deadline = time.time() + (float(args[0]) / 1000.0 if args else 60.0)
            time.sleep(0.2)
            while True:
                d = probe.load().get("doc", {})
                if not d.get("busy") and not d.get("measuring"):
                    break
                if time.time() > deadline:
                    raise RuntimeError("timed out waiting for idle")
                time.sleep(0.05)
            time.sleep(0.2)
        elif op == "expect":
            path, want = args[0], args[1]
            tol = float(args[2]) if len(args) > 2 else None
            got = probe.doc_value(path)
            if want == "null":
                ok = got is None
            elif want in ("true", "false"):
                ok = got is (want == "true")
            elif tol is not None or isinstance(got, (int, float)):
                try:
                    ok = got is not None and abs(float(got) - float(want)) <= (tol or 0.0)
                except ValueError:
                    ok = False
            else:
                ok = str(got) == want
            status = "PASS" if ok else "FAIL"
            print(f"{status} expect {path} {want}{'' if tol is None else ' ±' + str(tol)} (got {got})", flush=True)
            if not ok:
                probe.failures.append(line)
            continue
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
    probe = Probe(os.path.join(state, "probe.json"))
    run(Driver(bus), lines, probe)
    if probe.failures:
        print(f"{len(probe.failures)} expectation(s) failed", flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
