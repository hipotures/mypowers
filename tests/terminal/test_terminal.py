import asyncio
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

import httpx
import pytest
from mypowers_tui.dashboard import Dashboard
from mypowers_tui.input import Decoder, InputEvent
from mypowers_tui.main import Application
from rich.console import Console

from mypowers.contracts import Status


def size(fd, width, height):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))


def read_until(fd, needle, budget=5):
    deadline = time.monotonic() + budget
    captured = bytearray()
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            try:
                raw = os.read(fd, 65536)
            except OSError:
                break
            captured.extend(raw)
            if needle in captured:
                break
    assert needle in captured, bytes(captured)[-3000:]
    return bytes(captured)


def test_fragmented_paste_mouse_escape_bounds():
    decoder = Decoder()
    assert decoder.feed(b"\x1b[<0;10;") == []
    assert decoder.feed(b"12M") == [InputEvent("press", x=10, y=12, button=0)]
    assert decoder.feed(b"\x1b[<0;10;12m") == [InputEvent("release", x=10, y=12, button=0)]
    assert decoder.feed(b"\x1b[200~ac on\nq") == []
    assert decoder.feed(b"\x1b[20") == []
    assert decoder.feed(b"1~d") == [InputEvent("key", "d")]
    assert decoder.feed(b"\x1b[Z\t\r ") == [
        InputEvent("key", "backtab"),
        InputEvent("key", "tab"),
        InputEvent("key", "enter"),
        InputEvent("key", "space"),
    ]
    assert decoder.feed(b"\x1b[999~") == []
    assert decoder.feed(b"\x1b[<0;1;1" + b"9" * 5000) == []
    assert not decoder.buffer


@pytest.mark.parametrize("width,height", [(100, 28), (80, 24), (60, 18), (40, 12), (30, 10)])
def test_adaptive_rendering_and_no_color(core, width, height):
    dashboard = Dashboard()
    dashboard.update(core[0].snapshot())
    console = Console(width=width, height=height, record=True, no_color=True)
    console.print(dashboard.render(width, height))
    captured = console.export_text()
    if width >= 40:
        assert "BATTERY" in captured and "INPUT" in captured and "OUTPUT" in captured
        assert all(hit.x2 <= width and hit.y <= height for hit in dashboard.hits)
    else:
        assert "Resize terminal" in captured
    assert "\x1b" not in captured


async def test_mouse_release_once_layout_guard_pending_stale_and_keyboard(core):
    dashboard = Dashboard()
    dashboard.update(core[0].snapshot())
    dashboard.render(100, 28)

    class FakeClient:
        def __init__(self):
            self.calls = []

        async def admit(self, output, enabled, snapshot):
            self.calls.append((output, enabled))
            raise __import__("mypowers.contracts", fromlist=["AppError"]).AppError("test", "denied")

    client = FakeClient()
    app = Application(client, dashboard, asyncio.Event())
    hit = dashboard.hits[0]
    app.event(InputEvent("press", x=hit.x1, y=hit.y))
    app.event(InputEvent("release", x=hit.x1, y=hit.y))
    app.event(InputEvent("release", x=hit.x1, y=hit.y))
    assert dashboard.pending
    app.activate("ac")
    await asyncio.sleep(0)
    assert len(client.calls) == 1
    assert not dashboard.pending
    app.event(InputEvent("press", x=hit.x1, y=hit.y))
    dashboard.render(80, 24)
    app.event(InputEvent("release", x=hit.x1, y=hit.y))
    assert len(client.calls) == 1
    dashboard.server_connected = False
    app.activate("dc")
    assert len(client.calls) == 1
    app.event(InputEvent("key", "tab"))
    app.event(InputEvent("key", "backtab"))
    app.event(InputEvent("key", "l"))
    assert dashboard.view == "logs"
    app.event(InputEvent("scroll", button=64))
    assert dashboard.scroll == 3
    app.event(InputEvent("key", "q"))
    assert app.stop.is_set()
    await app.close()


@pytest.mark.parametrize("ending", ["q", "sigterm", "ctrlc", "ctrlz"])
def test_real_pty_mouse_keyboard_resize_and_shell_restoration(daemon_process, ending):
    _, url, env = daemon_process
    env = {**env, "TERM": "xterm-256color", "NO_COLOR": "1"}
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    size(slave, 100, 28)
    exe = Path(sys.executable).parent / "mypowers-tui"
    # exec remains followed by a real shell read, checking that echo/input work after restoration.
    script = '"$1" --server "$2"; IFS= read -r line; printf "\\nSHELL_ECHO:%s\\n" "$line"'

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(
        ["bash", "-c", script, "bash", str(exe), url],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        preexec_fn=controlling_terminal,
    )
    try:
        captured = read_until(master, b"DATA LIVE")
        with httpx.Client(base_url=url, trust_env=False) as client:
            snapshot = Status.model_validate(client.get("/api/v1/status").json())
            dashboard = Dashboard()
            dashboard.update(snapshot)
            dashboard.render(100, 28)
            ac = dashboard.hits[0]
            mouse = f"\x1b[<0;{ac.x1};{ac.y}M\x1b[<0;{ac.x1};{ac.y}m".encode()
            os.write(master, mouse[:5])
            time.sleep(0.05)
            os.write(master, mouse[5:])
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline:
                if client.get("/api/v1/status").json()["telemetry"]["sample"]["ac_enabled"]:
                    break
                time.sleep(0.05)
            else:
                pytest.fail("Mouse did not submit AC intention through HTTP.")
            captured += read_until(master, b"AC confirmed |")
            os.write(master, b"\t\r")
            captured += read_until(master, b"DC confirmed")
            assert client.get("/api/v1/status").json()["telemetry"]["sample"]["dc_enabled"]
            # Paste must never execute q or further output changes.
            os.write(master, b"\x1b[200~q\nac off\x1b[201~")
            size(slave, 80, 24)
            os.killpg(process.pid, signal.SIGWINCH)
            time.sleep(0.3)
            os.write(master, b"l")
            captured += read_until(master, b"LOGS")
            os.write(master, b"d")
            captured += read_until(master, b"BATTERY")
        if ending == "sigterm":
            # Find only this shell's child, without signaling the shell that must read afterward.
            child = int(
                Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()[0]
            )
            os.kill(child, signal.SIGTERM)
        else:
            os.write(master, {"q": b"q", "ctrlc": b"\x03", "ctrlz": b"\x1a"}[ending])
        captured += read_until(master, b"\x1b[?2004l")
        time.sleep(0.1)
        assert termios.tcgetattr(slave) == original
        os.write(master, b"normal-echo\n")
        captured += read_until(master, b"SHELL_ECHO:normal-echo")
        assert b"\x1b[?1000l" in captured and b"\x1b[?1006l" in captured
        assert b"\x1b[?25h" in captured and b"\x1b[?1049l" in captured
        assert process.wait(timeout=3) == 0
        assert b"Traceback" not in captured
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=3)
        os.close(master)
        os.close(slave)
