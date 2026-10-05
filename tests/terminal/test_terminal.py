"""Exercise the native Rust TUI against an isolated simulated daemon in real PTYs."""

import codecs
import fcntl
import json
import os
import pty
import select
import shlex
import signal
import socket
import sqlite3
import struct
import subprocess
import termios
import threading
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import httpx
import pyte
import pytest

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "frontends/tui/target/debug/mypowers-tui"
CADDY_BINARY = os.environ.get("MYPOWERS_CADDY_TEST_BINARY")


@pytest.fixture(scope="session")
def tui_binary():
    assert BINARY.is_file(), (
        "Build first: cargo build --locked --manifest-path frontends/tui/Cargo.toml"
    )
    return BINARY


def size(fd, width, height):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))


def read_until(fd, needle, budget=8, session=None):
    deadline = time.monotonic() + budget
    captured = bytearray()
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            try:
                raw = os.read(fd, 65536)
                captured.extend(raw)
                if session is not None:
                    height, width, _, _ = struct.unpack(
                        "HHHH", fcntl.ioctl(session.slave, termios.TIOCGWINSZ, b"\0" * 8)
                    )
                    session.screen.resize(lines=height, columns=width)
                    session.stream.feed(session.decoder.decode(raw))
            except OSError:
                break
            if needle in captured or (
                session is not None
                and needle.decode(errors="ignore") in "\n".join(session.screen.display)
            ):
                return bytes(captured)
    pytest.fail(f"Missing {needle!r}: {bytes(captured)[-3000:]!r}")


class Session:
    def __init__(self, binary, tmp_path, env, *arguments):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        size(self.slave, 94, 24)
        self.screen = pyte.Screen(94, 24)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [str(binary), *arguments],
            cwd=tmp_path,
            stdin=self.slave,
            stdout=self.slave,
            stderr=self.slave,
            env={**env, "TERM": "xterm-256color"},
            preexec_fn=controlling_terminal,
        )

    def write(self, data):
        os.write(self.master, data)

    def read(self, needle, budget=8):
        return read_until(self.master, needle, budget, self)

    def close(self):
        if self.process.poll() is None:
            self.write(b"\x11")  # Ctrl-Q bypasses quit confirmation.
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                self.process.wait(timeout=3)
        assert termios.tcgetattr(self.slave) == self.original
        os.close(self.master)
        os.close(self.slave)


def wait_state(client, output, enabled):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        status = client.get("/api/v1/status").json()
        if (
            status["telemetry"]["sample"][output + "_enabled"] == enabled
            and status["controls"]["allowed"]
        ):
            return status
        time.sleep(0.1)
    pytest.fail(f"No observed {output}={enabled}")


def empty_graph_history(connection, request):
    """Minimal history contract for fixtures that exercise only stream traffic."""
    if request.path.startswith("/api/v1/history/aggregates?"):
        query = parse_qs(urlsplit(request.path).query)
        return connection.respond(
            200,
            json.dumps(
                {
                    "schema_version": 1,
                    "source": "database",
                    "bucket_seconds": int(query["bucket_seconds"][0]),
                    "since_ms": int(datetime.fromisoformat(query["since"][0]).timestamp() * 1000),
                    "until_ms": int(datetime.fromisoformat(query["until"][0]).timestamp() * 1000),
                    "items": [],
                }
            ),
        )
    return None


def test_startup_backfills_the_live_graph_window_from_server_history(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    with httpx.Client(base_url=url, trust_env=False) as client:
        snapshot = client.get("/api/v1/status").json()
        sample = snapshot["telemetry"]["sample"]
        now_ms = int(datetime.fromisoformat(snapshot["server_time"]).timestamp() * 1000)
        # Seed only the fresh simulated fixture database, never a user database.
        with sqlite3.connect(tmp_path / "data" / "mypowers.db") as database:
            device_id = database.execute("SELECT id FROM devices LIMIT 1").fetchone()[0]
            database.executemany(
                "INSERT INTO telemetry(device_id,received_at_ms,segment_id,battery_percent,"
                "input_power_w,output_power_w,remaining_minutes,ac_enabled,dc_enabled,"
                "light_enabled,status_flags) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                [
                    (
                        device_id,
                        now_ms - offset * 1000,
                        sample["segment_id"],
                        71,
                        35,
                        3,
                        2880,
                        int(sample["ac_enabled"]),
                        int(sample["dc_enabled"]),
                        int(sample["light_enabled"]),
                        sample["status_flags"],
                    )
                    for offset in range(425, 4, -10)
                ],
            )
        page = client.get(
            "/api/v1/history",
            params={
                "since": datetime.fromtimestamp((now_ms - 430_000) / 1000, UTC).isoformat(),
                "until": snapshot["server_time"],
                "limit": 1000,
            },
        ).json()
        assert len(page["items"]) >= 43
    session = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        session.read(b"CONNECTED")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if select.select([session.master], [], [], 0.1)[0]:
                session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
            rows = session.screen.display
            label_row = next(i for i, row in enumerate(rows) if "INPUT" in row and "OUTPUT" in row)
            graph = rows[label_row + 2 : label_row + 4]
            counts = [
                sum(char in "▁▂▃▄▅▆▇█" for row in graph for char in row[start:end])
                for start, end in [(2, 45), (48, 92)]
            ]
            if min(counts) >= 30:
                break
        else:
            pytest.fail(
                f"Historical graphs did not fill their 10-second bucket window: {counts}\n"
                + "\n".join(rows)
            )
        # Historical 35 W / 3 W cannot overwrite the simulated live numeric readings.
        assert "INPUT 35 W" not in rows[label_row]
        assert "OUTPUT 3 W" not in rows[label_row]
        # The actual client rotates per-bar aggregates without replacing live numbers.
        for label, minimum, maximum in [("60s", 1, 10), ("1h", 1, 3), ("10s", 30, 43)]:
            session.write(b"t")
            session.read(f"t {label}".encode())
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if select.select([session.master], [], [], 0.1)[0]:
                    session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
                rows = session.screen.display
                label_row = next(
                    i for i, row in enumerate(rows) if "INPUT" in row and "OUTPUT" in row
                )
                graph = rows[label_row + 2 : label_row + 4]
                counts = [
                    sum(char in "▁▂▃▄▅▆▇█" for row in graph for char in row[start:end])
                    for start, end in [(2, 45), (48, 92)]
                ]
                if all(minimum <= count <= maximum for count in counts):
                    break
            else:
                pytest.fail(f"Unexpected {label} aggregate graph: {counts}")
            assert "INPUT 0 W" in rows[label_row] and "OUTPUT 0 W" in rows[label_row]
        # The same real client switches to a full-width shared Braille Chart locally.
        session.write(b"g")
        session.read(b"g spark")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if select.select([session.master], [], [], 0.1)[0]:
                session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
            rows = session.screen.display
            label_row = next(i for i, row in enumerate(rows) if "INPUT" in row and "OUTPUT" in row)
            graph = rows[label_row + 2 : label_row + 9]
            dots = sum("\u2801" <= char <= "\u28ff" for row in graph for char in row)
            if dots >= 30:
                break
        else:
            pytest.fail("Shared chart did not render persisted averages\n" + "\n".join(rows))
        assert "0–100 W" not in "\n".join(rows)
        assert "   100│" in "\n".join(rows) and "    50│" in "\n".join(rows)
        assert "     0└" in rows[label_row + 9]
        assert rows[label_row + 10].count(":") >= 3
        assert "INPUT 0 W" in rows[label_row] and "OUTPUT 0 W" in rows[label_row]
        session.write(b"g")
        session.read(b"g chart")
        session.write(b"\x11")
        session.process.wait(timeout=3)
        assert session.process.returncode == 0
    finally:
        session.close()


@pytest.mark.parametrize("ending", ["q", "sigterm", "sigint", "ctrlq"])
def test_native_controls_logs_paste_resize_and_restoration(
    daemon_process, tui_binary, tmp_path, ending
):
    _, url, env = daemon_process
    (tmp_path / ".env").write_text(f"MYPOWERS_SERVER_URL={url}\n")
    session = Session(tui_binary, tmp_path, env, "--no-color")
    try:
        session.read(b"CONNECTED")
        assert "INPUT" in "\n".join(session.screen.display) and "OUTPUT" in "\n".join(
            session.screen.display
        )
        with httpx.Client(base_url=url, trust_env=False) as client:
            row = next(
                index
                for index, text in enumerate(session.screen.display)
                if "AC" in text and "DC" in text
            )
            column = session.screen.display[row].index("AC")
            mouse = f"\x1b[<0;{column + 1};{row + 1}M\x1b[<0;{column + 1};{row + 1}m".encode()
            session.write(mouse[:5])
            time.sleep(0.05)
            session.write(mouse[5:])
            wait_state(client, "ac", True)
            session.read(b"confirmed")
            session.write(b"\t\r")
            wait_state(client, "dc", True)
            session.read(b"confirmed")
            # Paste is ignored, including quit and control shortcuts.
            session.write(b"\x1b[200~qadl\x1b[201~")
            time.sleep(0.3)
            assert session.process.poll() is None
            assert not client.get("/api/v1/status").json()["telemetry"]["sample"]["light_enabled"]
            size(session.slave, 50, 15)
            os.kill(session.process.pid, signal.SIGWINCH)
            session.read(b"Terminal too small")
            size(session.slave, 60, 19)
            os.kill(session.process.pid, signal.SIGWINCH)
            session.read(b"INPUT")
            session.write(b"\x1bOR")  # F3
            session.read(b"LOGS")
            session.write(b"f\x1b[A\x1b[Fb")
            session.read(b"Log level changed to DEBUG")
            assert "Runtime log level updated" not in "\n".join(session.screen.display)
            session.write(b"\x1b")  # Esc closes the modal.
            session.read(b"INPUT")
        if ending == "sigterm":
            session.process.terminate()
        elif ending == "sigint":
            session.process.send_signal(signal.SIGINT)
        else:
            if ending == "q":
                session.write(b"q")
                session.read(b"Quit MyPowers?")
                assert session.process.poll() is None
                session.write(b"\x1b")
                session.read(b"INPUT")
                assert session.process.poll() is None
                session.write(b"q")
                session.read(b"Quit MyPowers?")
                session.write(b"\r")
            else:
                session.write(b"\x11")  # Ctrl-Q
        restored = session.read(b"\x1b[?2004l")
        assert b"\x1b[?1006l" in restored
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()


def test_keyboard_event_types_and_modifiers_do_not_dispatch_shortcuts(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url, "--utc")

    def key(codepoint, modifiers=1, kind=1):
        return f"\x1b[{codepoint};{modifiers}:{kind}u".encode()

    def ignored_keys(characters):
        return (
            b"".join(
                key(ord(char), modifiers, kind)
                for char in characters
                for modifiers, kind in [(1, 2), (1, 3), (3, 1), (5, 1)]
                if not (char == "q" and modifiers == 5 and kind == 1)
            )
            + key(ord("q"), 5, 2)
            + key(ord("q"), 5, 3)
        )

    try:
        session.read(b"CONNECTED")
        with httpx.Client(base_url=url, trust_env=False) as client:
            before = client.get("/api/v1/status").json()
            session.write(ignored_keys("adlprsq?") + b"\x1b[13;1:2~\x1b[13;1:3~")
            # A real press is an ordered barrier after all ignored events.
            session.write(key(ord("?")))
            session.read("HELP — DASHBOARD".encode())
            session.write(key(27))
            session.read(b"F3 logs")
            session.write(b"\x1b[13;1:1~")  # F3 press.
            session.read(b"ARCHIVE")
            session.read(b"Log: INFO")
            session.write(ignored_keys("fb+-q?"))
            session.write(key(ord("?")))
            session.read("HELP — LOGS".encode())
            assert session.process.poll() is None
            after = client.get("/api/v1/status").json()
            assert after["connection"]["session_id"] == before["connection"]["session_id"]
            assert after["connection"]["desired"] == before["connection"]["desired"]
            assert after["controls"]["outputs_revision"] == before["controls"]["outputs_revision"]
            assert after["logging"]["effective_level"] == "INFO"
            for output in ["ac_enabled", "dc_enabled", "light_enabled"]:
                assert after["telemetry"]["sample"][output] == before["telemetry"]["sample"][output]
            session.write(key(27))
            session.read(b"F3 logs")
            session.write(key(ord("a")))
            session.read(b"AC ON confirmed")
            wait_state(client, "ac", True)
        session.write(key(ord("q"), 5))  # Ctrl-Q press still quits immediately.
        session.read(b"\x1b[?1049l")
        assert session.process.wait(timeout=3) == 0
        assert termios.tcgetattr(session.slave) == session.original
    finally:
        session.close()


def test_keyboard_burst_does_not_starve_immediate_quit(daemon_process, tui_binary, tmp_path):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        session.read(b"CONNECTED")
        # Focus changes are local and cannot send hardware commands.
        payload = b"\t" * 2048 + b"\x11"
        assert os.write(session.master, payload) == len(payload)
        # A terminal emulator drains output while the application processes input.
        session.read(b"\x1b[?1049l", budget=2)
        assert session.process.wait(timeout=2) == 0
        assert termios.tcgetattr(session.slave) == session.original
    finally:
        session.close()


@pytest.mark.parametrize("ending", ["ctrlq", "sigterm"])
def test_stream_bursts_do_not_starve_modal_resize_or_quit(
    daemon_process, tui_binary, tmp_path, ending
):
    from websockets.exceptions import ConnectionClosed
    from websockets.sync.server import serve

    _, url, env = daemon_process
    with httpx.Client(base_url=url, trust_env=False) as client:
        snapshot = client.get("/api/v1/status").json()
    stopped = threading.Event()
    sent = {"logs": 0, "status": 0}

    def stream(connection):
        logs = connection.request.path.startswith("/api/v1/logs/stream")
        current = json.loads(json.dumps(snapshot))
        sequence = 1
        try:
            connection.send(
                json.dumps(
                    {
                        "schema_version": 1,
                        "server_instance_id": snapshot["server_instance_id"],
                        "server_time": current["server_time"],
                        "stream_sequence": sequence,
                        "type": "snapshot",
                        "data": current,
                    }
                )
            )
            while not stopped.is_set():
                sequence += 1
                stamp = datetime.now(UTC).isoformat()
                if logs:
                    data = {
                        "schema_version": 1,
                        "timestamp": stamp,
                        "server_instance_id": snapshot["server_instance_id"],
                        "sequence": sequence,
                        "level": "INFO" if (sequence - 1) % 2048 == 0 else "DEBUG",
                        "message": f"Stream burst processed {sequence - 1}",
                    }
                else:
                    current["server_time"] = stamp
                    current["telemetry"]["sample"].update(sequence=sequence, received_at=stamp)
                    data = current
                connection.send(
                    json.dumps(
                        {
                            "schema_version": 1,
                            "server_instance_id": snapshot["server_instance_id"],
                            "server_time": stamp,
                            "stream_sequence": sequence,
                            "type": "log" if logs else "state",
                            "data": data,
                        }
                    )
                )
                sent["logs" if logs else "status"] += 1
                # Send an initial burst, then sustain traffic until the client exits.
                if logs and sequence > 2049 and sequence % 8 == 0:
                    stopped.wait(0.002)
                elif not logs:
                    stopped.wait(0.01)
        except ConnectionClosed:
            pass

    with serve(
        stream,
        "127.0.0.1",
        0,
        process_request=empty_graph_history,
        compression=None,
        close_timeout=1,
    ) as server:
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        session = None
        try:
            origin = f"http://127.0.0.1:{server.socket.getsockname()[1]}"
            session = Session(tui_binary, tmp_path, env, "--server", origin)
            session.read(b"CONNECTED")
            # This marker is emitted only after 2,048 individual log events.
            session.read(b"Stream burst processed 2048")
            before = sent.copy()
            session.write(b"?")
            session.read("HELP — DASHBOARD".encode(), budget=2)
            size(session.slave, 50, 15)
            os.kill(session.process.pid, signal.SIGWINCH)
            session.read(b"Terminal too small", budget=2)
            size(session.slave, 80, 24)
            os.kill(session.process.pid, signal.SIGWINCH)
            session.read("HELP — DASHBOARD".encode(), budget=2)
            # Observe consumption, not just sends: socket backpressure can stall a producer.
            session.read(b"Stream burst processed 4096")
            assert sent["status"] > before["status"]
            assert sent["logs"] >= 4096
            started = time.monotonic()
            if ending == "ctrlq":
                session.write(b"\x11")
            else:
                session.process.terminate()
            session.read(b"\x1b[?1049l", budget=2)
            assert session.process.wait(timeout=2) == 0
            assert time.monotonic() - started < 2
            assert termios.tcgetattr(session.slave) == session.original
        finally:
            stopped.set()
            if session is not None:
                session.close()
            server.shutdown()
            worker.join(timeout=2)
            assert not worker.is_alive()


def test_wide_station_names_clear_in_the_actual_terminal_stream(
    daemon_process, tui_binary, tmp_path
):
    from websockets.exceptions import ConnectionClosed
    from websockets.sync.server import serve

    _, url, env = daemon_process
    with httpx.Client(base_url=url, trust_env=False) as client:
        snapshot = client.get("/api/v1/status").json()
    names = ["電源電源電源", "NARROW", "界界界界界界", "OK"]
    stage = [0]
    stopped = threading.Event()

    def stream(connection):
        logs = connection.request.path.startswith("/api/v1/logs/stream")
        current = json.loads(json.dumps(snapshot))
        sequence = 0
        try:
            while not stopped.is_set():
                sequence += 1
                index = stage[0]
                stamp = datetime.now(UTC).isoformat()
                current["device"]["name"] = names[index]
                current["server_time"] = stamp
                current["telemetry"]["sample"].update(
                    sequence=sequence, received_at=stamp, battery_percent=70 + index
                )
                connection.send(
                    json.dumps(
                        {
                            "schema_version": 1,
                            "server_instance_id": snapshot["server_instance_id"],
                            "server_time": stamp,
                            "stream_sequence": sequence,
                            "type": "snapshot"
                            if sequence == 1
                            else "heartbeat"
                            if logs
                            else "state",
                            "data": None if logs and sequence > 1 else current,
                        }
                    )
                )
                stopped.wait(0.05)
        except ConnectionClosed:
            pass

    with serve(
        stream,
        "127.0.0.1",
        0,
        process_request=empty_graph_history,
        compression=None,
        close_timeout=1,
    ) as server:
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        session = None
        try:
            origin = f"http://127.0.0.1:{server.socket.getsockname()[1]}"
            session = Session(tui_binary, tmp_path, env, "--server", origin)

            def read_frame(percent):
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    if select.select([session.master], [], [], 0.1)[0]:
                        raw = os.read(session.master, 65536)
                        session.stream.feed(session.decoder.decode(raw))
                        rows = [
                            "".join(session.screen.buffer[y][x].data for x in range(94))
                            for y in range(24)
                        ]
                        if any(percent in line for line in rows):
                            return rows
                pytest.fail(f"Missing battery frame {percent}")

            rows = read_frame("70%")
            row = next(y for y, line in enumerate(rows) if names[0] in line)
            column = rows[row].index("電")
            for index, name in enumerate(names[1:], 1):
                stage[0] = index
                # The later battery render acts as a frame barrier for the preceding header.
                rows = read_frame(f"{70 + index}%")
                assert name in rows[row]
                if name.isascii():
                    anchor = session.screen.buffer[row][column]
                    # Pyte does not erase wide-cell stubs when their head is overwritten.
                    # Verify actual emitted head cells without normalizing its backing array.
                    for x in range(column + len(name), column + 12, 2):
                        cell = session.screen.buffer[row][x]
                        assert cell.data == " "
                        assert cell.fg == anchor.fg and cell.bg == anchor.bg
                        assert cell.bold == anchor.bold and cell.italics == anchor.italics
                    assert not any(character in rows[row] for character in "電源界")
            session.write(b"\x11")
            # Cleanup is an ANSI-stream assertion; Pyte's wide-cell display is unsupported here.
            read_until(session.master, b"\x1b[?1049l", budget=2)
            assert session.process.wait(timeout=2) == 0
            assert termios.tcgetattr(session.slave) == session.original
        finally:
            stopped.set()
            if session is not None:
                session.close()
            server.shutdown()
            worker.join(timeout=2)
            assert not worker.is_alive()


def test_native_pause_resume_retry_and_runtime_logging_receipts(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url, "--utc")
    try:
        session.read(b"CONNECTED")
        with httpx.Client(base_url=url, trust_env=False) as client:
            initial = client.get("/api/v1/status").json()
            session.write(b"s")
            session.read(b"SETTINGS")
            session.write(b"\t\t\t\t")
            session.read(b"Debug / Diagnostics")
            for key, message in [
                (b"p", b"Station connection paused"),
                (b"r", b"Reconnecting to station"),
                (b"p", b"CONNECTED"),
            ]:
                session.write(key)
                captured = session.read(message)
                assert b"Request failed" not in captured
                desired = "running" if message == b"CONNECTED" else "paused"
                assert client.get("/api/v1/status").json()["connection"]["desired"] == desired
            session.read(b"Log: INFO")
            for level in ["DEBUG", "INFO"]:
                session.write(b"b")
                captured = session.read(f"Log level changed to {level}".encode())
                assert b"Request failed" not in captured
                session.read(f"Log: {level}".encode())
                health = client.get("/api/v1/status").json()["logging"]
                assert health["effective_level"] == level
                assert health["override_expires_at"] is None
            final = client.get("/api/v1/status").json()
            assert final["server_instance_id"] == initial["server_instance_id"]
            assert final["connection"]["desired"] == "running"
            for output in ["ac_enabled", "dc_enabled", "light_enabled"]:
                assert (
                    final["telemetry"]["sample"][output] == initial["telemetry"]["sample"][output]
                )
    finally:
        session.close()


def test_status_strip_confirmations_fade_and_debug_does_not_keep_it_alive(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        session.read(b"CONNECTED")
        session.write(b"a")
        session.read(b"AC ON confirmed")
        border = next(index for index, row in enumerate(session.screen.display) if "q quit" in row)
        assert session.screen.display[border].startswith("╰")
        assert session.screen.display[border + 1].strip() == "AC ON confirmed"
        assert not any(
            value in "\n".join(session.screen.display)
            for value in ["Age ", "History ok", "hci", "Log INFO"]
        )
        with httpx.Client(base_url=url, trust_env=False) as client:
            wait_state(client, "ac", True)
            client.put("/api/v1/runtime/log-level", json={"level": "DEBUG"}).raise_for_status()
            session.read(b"Log level changed to DEBUG")
            page = client.get("/api/v1/logs", params={"tail": 100}).json()
            audit = [row for row in page["items"] if row.get("event") == "log_level_changed"]
            assert audit and audit[-1]["level"] == "INFO"
        time.sleep(8.5)
        session.read(b"CONNECTED")
        assert session.screen.display[border + 1].strip() == ""
        assert session.process.poll() is None
        session.write(b"d")
        session.read(b"DC ON confirmed")
        assert session.screen.display[border + 1].strip() == "DC ON confirmed"
    finally:
        session.close()


def test_modal_shortcuts_do_not_dispatch_dashboard_actions(daemon_process, tui_binary, tmp_path):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url, "--utc")
    try:
        session.read(b"CONNECTED")
        session.write(b"\x1bOR")  # F3
        session.read(b"LOGS")
        session.read(b"Log: INFO")
        with httpx.Client(base_url=url, trust_env=False) as client:
            before = client.get("/api/v1/status").json()
            session.write(b"rpadl\x1bOQ\x1bOR\x1b[15~\x03\x1a")
            time.sleep(0.5)
            session.read(b"ARCHIVE")
            assert session.process.poll() is None
            screen = "\n".join(session.screen.display)
            assert "refresh" not in screen and "F2" not in screen and "F3" not in screen
            after = client.get("/api/v1/status").json()
            assert after["connection"]["session_id"] == before["connection"]["session_id"]
            assert after["connection"]["desired"] == before["connection"]["desired"]
            for output in ["ac_enabled", "dc_enabled", "light_enabled"]:
                assert after["telemetry"]["sample"][output] == before["telemetry"]["sample"][output]
            session.write(b"b")
            session.read(b"Log level changed to DEBUG")
            session.read(b"Log: DEBUG")
            session.write(b"?")
            session.read("HELP — LOGS".encode())
            session.write(b"rpadlb\x1bOR")
            time.sleep(0.5)
            session.read("HELP — LOGS".encode())
            assert client.get("/api/v1/status").json()["logging"]["effective_level"] == "DEBUG"
            assert session.process.poll() is None
        session.write(b"\x1b")
        session.read(b"F3 logs")
        assert "F3 logs" in "\n".join(session.screen.display)
    finally:
        session.close()


def test_day_archive_lazy_pages_drag_and_live_resume(daemon_process, tui_binary, tmp_path):
    _, url, env = daemon_process
    helpers = tmp_path / "helpers"
    helpers.mkdir()
    captured = tmp_path / "clipboard.txt"
    helper = helpers / "wl-copy"
    helper.write_text("#!/bin/sh\ncat > " + shlex.quote(str(captured)) + "\n")
    helper.chmod(0o755)
    env = {**env, "PATH": str(helpers) + os.pathsep + env["PATH"]}
    today = datetime.now(UTC).replace(hour=0, minute=0, second=0, microsecond=0)
    yesterday = today - timedelta(days=1)
    records = [
        {
            "schema_version": 1,
            "timestamp": (date + timedelta(seconds=index)).isoformat(),
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "sequence": sequence,
            "level": "INFO",
            "message": f"{prefix} {index:03}",
        }
        for date, prefix, count, base in [
            (yesterday, "Yesterday", 10, 0),
            (today, "Archive", 205, 10),
        ]
        for index in range(count)
        for sequence in [base + index]
    ]
    # Only the simulated daemon's temporary retained file is populated.
    (tmp_path / "logs/mypowers.jsonl.1").write_text(
        "".join(json.dumps(record) + "\n" for record in records)
    )
    session = Session(tui_binary, tmp_path, env, "--server", url, "--utc", "--no-color")
    try:
        session.read(b"CONNECTED")
        session.write(b"\x1bOR")
        session.read(b"Archive 000")
        session.read(b"ARCHIVE")
        session.write(b"-")
        session.read(b"page 50")
        session.read(b"50 loaded")
        assert "Archive 000" in "\n".join(session.screen.display)
        row = next(index for index, text in enumerate(session.screen.display) if " LOGS " in text)
        column = session.screen.display[row].index("LOGS") + 1
        click = f"\x1b[<0;{column + 1};{row + 1}M\x1b[<0;{column + 1};{row + 1}m".encode()
        session.write(click)
        time.sleep(0.15)
        assert not captured.exists()
        session.write(click)
        session.read(b"Logs copied")
        copied = captured.read_text()
        assert len(copied.splitlines()) == 50
        assert "INFO Archive 000" in copied and "INFO Archive 049" in copied
        assert "Archive 050" not in copied
        visible = [row for row in session.screen.display if "Archive" in row]
        with httpx.Client(base_url=url, trust_env=False) as client:
            client.put("/api/v1/runtime/log-level", json={"level": "DEBUG"}).raise_for_status()
        session.read(b"new")
        assert visible == [row for row in session.screen.display if "Archive" in row]
        thumbs = [
            (x, y)
            for y, row in enumerate(session.screen.display)
            for x, symbol in enumerate(row)
            if symbol == "█"
        ]
        x = max(x for x, _ in thumbs)
        y = min(y for column, y in thumbs if column == x)
        bottom = max(
            row
            for row in range(session.screen.lines)
            if session.screen.display[row][x] in {"█", "│"}
        )
        session.write(f"\x1b[<0;{x + 1};{y + 1}M".encode())
        session.write(f"\x1b[<32;{x + 1};{bottom + 1}M".encode())
        session.write(f"\x1b[<0;{x + 1};{bottom + 1}m".encode())
        session.read(b"100 loaded")
        session.write(b"\x1b[6~")
        session.read(b"Archive 050")
        session.write(b"\x1b[H")  # Home loads the selected day's beginning.
        session.read(b"50 loaded")
        session.read(b"Archive 000")
        session.write(b"\x1b[D")
        session.read(b"Yesterday 009")
        assert yesterday.date().isoformat() in "\n".join(session.screen.display)
        session.write(b"\x1b[B")
        session.read(b"Archive 000")
        session.write(b"\x1b[A")  # Start of today's range -> previous day.
        session.read(b"Yesterday 009")
        session.write(b"\x1b[F")
        session.read(b"Received station frame")
        assert today.date().isoformat() in "\n".join(session.screen.display)
        assert "LIVE | UTC" in "\n".join(session.screen.display)
        session.write(b"?")
        session.read(b"HELP")
        session.write(b"\x1b")
        session.read(b"INPUT")
        assert session.process.poll() is None
        session.write(b"\x1b")
        session.read(b"INPUT")
        assert session.process.poll() is None
    finally:
        session.close()


def test_failed_live_archive_refresh_is_throttled_and_end_still_retries_immediately(
    daemon_process, tui_binary, tmp_path
):
    from websockets.exceptions import ConnectionClosed
    from websockets.sync.server import serve

    _, url, env = daemon_process
    with httpx.Client(base_url=url, trust_env=False) as client:
        snapshot = client.get("/api/v1/status").json()
    stopped = threading.Event()
    emit_logs = threading.Event()
    queries = []

    def envelope(sequence, kind, data):
        return json.dumps(
            {
                "schema_version": 1,
                "server_instance_id": snapshot["server_instance_id"],
                "server_time": datetime.now(UTC).isoformat(),
                "stream_sequence": sequence,
                "type": kind,
                "data": data,
            }
        )

    def record(sequence, message):
        return {
            "sequence": sequence,
            "timestamp": datetime.now(UTC).isoformat(),
            "server_instance_id": snapshot["server_instance_id"],
            "level": "DEBUG",
            "message": message,
        }

    def http(connection, request):
        history = empty_graph_history(connection, request)
        if history is not None:
            return history
        if request.path.startswith("/api/v1/logs?"):
            queries.append(time.monotonic())
            if len(queries) > 2:
                return connection.respond(503, '{"error":{"code":"temporarily_unavailable"}}')
            page = {
                "schema_version": 1,
                "items": [record(0, "Initial archive" if len(queries) == 1 else "Live baseline")],
                "previous_cursor": None,
                "next_cursor": None,
                "has_more_before": False,
                "has_more_after": False,
                "source": "files",
                "gap": False,
                "skipped_lines": 0,
            }
            return connection.respond(200, json.dumps(page))
        return None

    def stream(connection):
        logs = connection.request.path.startswith("/api/v1/logs/stream")
        current = json.loads(json.dumps(snapshot))
        sequence = 1
        try:
            connection.send(envelope(sequence, "snapshot", current))
            if logs:
                while not stopped.is_set() and not emit_logs.wait(0.05):
                    pass
                for index in range(600):
                    if stopped.is_set():
                        return
                    sequence += 1
                    connection.send(envelope(sequence, "log", record(index + 1, "Live history")))
            while not stopped.wait(0.05):
                sequence += 1
                current["server_time"] = datetime.now(UTC).isoformat()
                current["telemetry"]["sample"].update(
                    sequence=sequence, received_at=current["server_time"]
                )
                connection.send(envelope(sequence, "heartbeat" if logs else "state", current))
        except ConnectionClosed:
            pass

    with serve(
        stream, "127.0.0.1", 0, process_request=http, compression=None, close_timeout=1
    ) as server:
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        session = None
        try:
            origin = f"http://127.0.0.1:{server.socket.getsockname()[1]}"
            session = Session(tui_binary, tmp_path, env, "--server", origin, "--utc")
            session.read(b"CONNECTED")
            session.write(b"\x1bOR")
            session.read(b"Initial archive")
            session.write(b"\x1b[F")
            session.read(b"Live baseline")
            emit_logs.set()
            session.read(b"Could not load logs")
            assert len(queries) == 3
            # Even a busy input loop must not turn a failed automatic query into a retry storm.
            session.write(b"\t" * 2048 + b"?")
            session.read("HELP — LOGS".encode(), budget=2)
            deadline = time.monotonic() + 0.7
            while time.monotonic() < deadline:
                if select.select([session.master], [], [], 0.05)[0]:
                    session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
            assert len(queries) == 3
            deadline = time.monotonic() + 3
            while len(queries) == 3 and time.monotonic() < deadline:
                if select.select([session.master], [], [], 0.05)[0]:
                    session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
            assert len(queries) == 4, "Automatic refresh must eventually retry"
            assert queries[3] - queries[2] >= 2
            # Esc returns to the dashboard; reopening Logs preserves its current range.
            session.write(b"\x1b")
            session.read(b"F3 logs")
            session.write(b"\x1bOR")
            session.read(b"LIVE | UTC")
            session.write(b"\x1b[F")
            deadline = time.monotonic() + 1
            while len(queries) < 5 and time.monotonic() < deadline:
                if select.select([session.master], [], [], 0.05)[0]:
                    session.stream.feed(session.decoder.decode(os.read(session.master, 65536)))
            assert len(queries) == 5, "End must retry without waiting for the automatic delay"
            assert queries[4] - queries[3] < 1.5
            session.write(b"\x11")
            session.read(b"\x1b[?1049l", budget=2)
            assert session.process.wait(timeout=2) == 0
        finally:
            stopped.set()
            if session is not None:
                session.close()
            server.shutdown()
            worker.join(timeout=2)
            assert not worker.is_alive()


def test_three_clients_share_state_and_do_not_stop_daemon(daemon_process, tui_binary, tmp_path):
    daemon, url, env = daemon_process
    sessions = [Session(tui_binary, tmp_path, env, "--server", url, "--no-mouse") for _ in range(3)]
    try:
        for session in sessions:
            session.read(b"CONNECTED")
        with httpx.Client(base_url=url, trust_env=False) as client:
            before = client.get("/api/v1/status").json()
            sessions[0].write(b"a")
            after = wait_state(client, "ac", True)
            for session in sessions:
                session.read(b"confirmed")
            assert before["server_instance_id"] == after["server_instance_id"]
            assert before["connection"]["session_id"] == after["connection"]["session_id"]
        sessions[0].close()
        sessions = sessions[1:]
        assert daemon.poll() is None
        with httpx.Client(base_url=url, trust_env=False) as client:
            assert client.get("/api/v1/status").json()["telemetry"]["sample"]["ac_enabled"]
    finally:
        for session in sessions:
            session.close()


def test_unreachable_unknown_values_exit_and_non_tty(tui_binary, tmp_path):
    env = {key: value for key, value in os.environ.items() if not key.startswith("MYPOWERS_")}
    session = Session(tui_binary, tmp_path, env, "--server", "http://127.0.0.1:1", "--no-mouse")
    try:
        session.read(b"DAEMON OFFLINE")
        session.read(b"--h --m")
        session.read(b"TLS")  # Wait for the first failed connection attempt before acting.
        text = "\n".join(session.screen.display)
        assert "--%" in text and "--h --m" in text
        session.write(b"a")
        session.read(b"Controls unavailable")
    finally:
        session.close()
    result = subprocess.run(
        [str(tui_binary)], cwd=tmp_path, env=env, capture_output=True, text=True
    )
    assert result.returncode == 2 and "requires a terminal" in result.stderr


def test_render_output_failure_restores_the_independent_input_terminal(tui_binary, tmp_path):
    input_master, input_slave = pty.openpty()
    output_master, output_slave = pty.openpty()
    original = termios.tcgetattr(input_slave)
    size(input_slave, 94, 24)
    size(output_slave, 94, 24)
    process = None

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(input_slave, termios.TIOCSCTTY, 0)

    try:
        # Keep both network workers on an isolated listener that never sends data.
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            process = subprocess.Popen(
                [
                    str(tui_binary),
                    "--server",
                    f"http://127.0.0.1:{listener.getsockname()[1]}",
                ],
                cwd=tmp_path,
                stdin=input_slave,
                stdout=output_slave,
                stderr=input_slave,
                env={"PATH": os.defpath, "TERM": "xterm-256color"},
                preexec_fn=controlling_terminal,
            )
            captured = read_until(output_master, b"MYPOWERS", budget=3)
            assert b"\x1b[?1049h" in captured
            assert process.poll() is None
            assert termios.tcgetattr(input_slave)[3] & (termios.ECHO | termios.ICANON) == 0

            # Fail only stdout after a real production frame; stdin stays inspectable.
            os.close(output_master)
            output_master = None
            os.write(input_master, b"\t")
            assert process.wait(timeout=3) == 2
            assert termios.tcgetattr(input_slave) == original
            diagnostic = read_until(input_master, b"terminal settings restored.", budget=1)
            assert b"TUI stopped after an I/O error" in diagnostic
            assert b"panicked at" not in diagnostic
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        for descriptor in [input_master, input_slave, output_master, output_slave]:
            if descriptor is not None:
                os.close(descriptor)


@pytest.mark.parametrize(
    "source,expected",
    [
        ("unrelated-value", b"TUI requires a terminal."),
        ("unrelated-key", b"TUI requires a terminal."),
        ("setting", b"MyPowers environment values must use UTF-8."),
        ("argument", b"Arguments must use UTF-8."),
    ],
)
def test_non_utf8_startup_input_never_panics_or_echoes_values(
    tui_binary, tmp_path, source, expected
):
    marker = b"ReviewFakeValue"
    raw = b"\xff" + marker
    env = {b"PATH": os.fsencode(os.defpath)}
    args = [os.fsencode(tui_binary)]
    if source == "unrelated-value":
        env[b"REVIEW_BINARY"] = raw
    elif source == "unrelated-key":
        env[raw] = b"ignored"
    elif source == "setting":
        env[b"MYPOWERS_API_TOKEN"] = raw
    else:
        args.extend([b"--server", raw])
    result = subprocess.run(args, cwd=tmp_path, env=env, capture_output=True, timeout=3)
    assert result.returncode == 2
    assert expected in result.stderr
    assert marker not in result.stdout + result.stderr
    assert b"panicked at" not in result.stderr
    assert b"\x1b[?1049h" not in result.stdout


@pytest.mark.parametrize("daemon_process", [True], indirect=True)
def test_native_authentication_and_private_relative_token_file(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    token = env["MYPOWERS_API_TOKEN"]
    env = {key: value for key, value in env.items() if key != "MYPOWERS_API_TOKEN"}
    selected = tmp_path / "settings"
    selected.mkdir()
    token_file = selected / "token"
    token_file.write_text(token)
    token_file.chmod(0o600)
    dotenv = selected / "client.env"
    dotenv.write_text(f"MYPOWERS_SERVER_URL={url}\nMYPOWERS_API_TOKEN_FILE=./token\n")
    session = Session(tui_binary, tmp_path, env, "--env-file", str(dotenv))
    try:
        session.read(b"CONNECTED")
        session.write(b"a")
        session.read(b"confirmed")
        assert token not in "\n".join(session.screen.display)
    finally:
        session.close()
    unauthorized = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        unauthorized.read(b"authentication")
        assert "DAEMON OFFLINE" in "\n".join(unauthorized.screen.display)
    finally:
        unauthorized.close()
    token_file.chmod(0o644)
    result = subprocess.run(
        [str(tui_binary), "--env-file", str(dotenv)],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 2 and "private permissions" in result.stderr
    assert token not in result.stderr


def test_closing_native_client_during_command_does_not_cancel_or_restore_outputs(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    session = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        session.read(b"CONNECTED")
        session.write(b"a")
        with httpx.Client(base_url=url, trust_env=False) as client:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                snapshot = client.get("/api/v1/status").json()
                if snapshot["controls"]["pending_command_id"]:
                    break
                time.sleep(0.02)
            else:
                pytest.fail("Command was not admitted")
            session.close()
            session = None
            wait_state(client, "ac", True)
    finally:
        if session is not None:
            session.close()


def test_native_https_wss_and_untrusted_ca(daemon_process, tui_binary, tmp_path):
    if CADDY_BINARY is None:
        pytest.skip("Set MYPOWERS_CADDY_TEST_BINARY for native HTTPS/WSS verification.")
    _, url, env = daemon_process
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    config = tmp_path / "Caddyfile"
    config.write_text(
        "{\n auto_https disable_redirects\n skip_install_trust\n}\n"
        f"https://localhost:{port} {{\n tls internal\n reverse_proxy {url}\n}}\n"
    )
    ca = tmp_path / "caddy-data/caddy/pki/authorities/local/root.crt"
    caddy_env = {
        **env,
        "XDG_DATA_HOME": str(tmp_path / "caddy-data"),
        "XDG_CONFIG_HOME": str(tmp_path / "caddy-config"),
    }
    with (tmp_path / "caddy.log").open("w") as log:
        process = subprocess.Popen(
            [CADDY_BINARY, "run", "--config", str(config), "--adapter", "caddyfile"],
            env=caddy_env,
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert process.poll() is None, (tmp_path / "caddy.log").read_text()
                if ca.exists():
                    break
                time.sleep(0.05)
            else:
                pytest.fail("Test CA not generated")
            trusted = Session(
                tui_binary,
                tmp_path,
                env,
                "--server",
                f"https://localhost:{port}",
                "--ca-file",
                str(ca),
            )
            try:
                trusted.read(b"CONNECTED")  # Verified WSS snapshot.
                trusted.write(b"a")
                trusted.read(b"confirmed")  # Verified HTTPS admission and result polling.
            finally:
                trusted.close()
            untrusted = Session(tui_binary, tmp_path, env, "--server", f"https://localhost:{port}")
            try:
                untrusted.read(b"TLS")
                assert "DAEMON OFFLINE" in "\n".join(untrusted.screen.display)
            finally:
                untrusted.close()
        finally:
            process.terminate()
            process.wait(timeout=5)


def test_title_double_click_copies_actual_snapshot_without_touching_desktop_clipboard(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    helpers = tmp_path / "helpers"
    helpers.mkdir()
    captured = tmp_path / "clipboard.json"
    helper = helpers / "wl-copy"
    helper.write_text("#!/bin/sh\ncat > " + shlex.quote(str(captured)) + "\n")
    helper.chmod(0o755)
    env = {**env, "PATH": str(helpers) + os.pathsep + env["PATH"]}
    session = Session(tui_binary, tmp_path, env, "--server", url)
    try:
        session.read(b"CONNECTED")
        row = next(index for index, text in enumerate(session.screen.display) if "MYPOWERS" in text)
        column = session.screen.display[row].index("MYPOWERS") + 3
        click = f"\x1b[<0;{column + 1};{row + 1}M\x1b[<0;{column + 1};{row + 1}m".encode()
        session.write(click)
        time.sleep(0.15)
        assert not captured.exists()
        session.write(click)
        session.read(b"JSON copied")
        snapshot = json.loads(captured.read_text())
        with httpx.Client(base_url=url, trust_env=False) as client:
            observed = client.get("/api/v1/status").json()
        assert snapshot["server_instance_id"] == observed["server_instance_id"]
        assert (
            snapshot["telemetry"]["sample"]["battery_percent"]
            == observed["telemetry"]["sample"]["battery_percent"]
        )
        assert "mock" not in snapshot
    finally:
        session.close()


@pytest.mark.parametrize("ending", ["timeout", "ctrlq"])
def test_stalled_clipboard_helper_is_stopped_without_blocking_the_tui(
    daemon_process, tui_binary, tmp_path, ending
):
    _, url, env = daemon_process
    helpers = tmp_path / "helpers"
    helpers.mkdir()
    pid_path = tmp_path / "clipboard.pid"
    helper = helpers / "wl-copy"
    helper.write_text(
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > " + shlex.quote(str(pid_path)) + "\nexec sleep 30\n"
    )
    helper.chmod(0o755)
    env = {**env, "PATH": str(helpers) + os.pathsep + env["PATH"]}
    session = Session(tui_binary, tmp_path, env, "--server", url)
    pid_fd = None
    try:
        session.read(b"CONNECTED")
        row = next(index for index, text in enumerate(session.screen.display) if "MYPOWERS" in text)
        column = session.screen.display[row].index("MYPOWERS") + 3
        click = f"\x1b[<0;{column + 1};{row + 1}M\x1b[<0;{column + 1};{row + 1}m".encode()
        session.write(click)
        time.sleep(0.15)
        session.write(click)
        deadline = time.monotonic() + 3
        while not pid_path.exists() or pid_path.stat().st_size == 0:
            assert time.monotonic() < deadline, "Clipboard helper did not start"
            time.sleep(0.02)
        # A pidfd identifies our helper even if the numeric PID is later reused.
        pid_fd = os.pidfd_open(int(pid_path.read_text()))
        if ending == "timeout":
            session.read(b"Copy failed; check clipboard access", budget=4)
            assert session.process.poll() is None
            session.write(b"\x11")
        else:
            started = time.monotonic()
            session.write(b"\x11")
        session.read(b"\x1b[?1049l", budget=1.5)
        assert session.process.wait(timeout=1.5) == 0
        if ending == "ctrlq":
            assert time.monotonic() - started < 1.5
        assert select.select([pid_fd], [], [], 3)[0], "Clipboard helper survived cancellation"
        assert termios.tcgetattr(session.slave) == session.original
    finally:
        if pid_fd is not None:
            try:
                signal.pidfd_send_signal(pid_fd, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.close(pid_fd)
        session.close()


def test_settings_startup_interval_is_saved_on_daemon_and_applied_on_next_tui_start(
    daemon_process, tui_binary, tmp_path
):
    _, url, env = daemon_process
    with httpx.Client(base_url=url, trust_env=False) as client:
        assert (
            client.put("/api/v1/settings", json={"graph_interval_seconds": 60}).status_code == 200
        )
        session = Session(tui_binary, tmp_path, env, "--server", url)
        try:
            session.read(b"t 60s")
            session.write(b"s")
            session.read(b"SETTINGS")
            session.write(b"\t")
            session.read(b"Power graphs")
            session.read(b"Startup interval")
            session.write(b"d")
            session.read(b"(unsaved)")
            session.write(b"s")
            session.read(b"Settings saved")
            assert client.get("/api/v1/settings").json()["graph_interval_seconds"] == 3600
            current = next(row for row in session.screen.display if "Current interval" in row)
            assert "60s" in current
            session.write(b"\x1b")
            session.read(b"t 60s")
        finally:
            session.close()
        restarted = Session(tui_binary, tmp_path, env, "--server", url)
        try:
            restarted.read(b"t 1h")
        finally:
            restarted.close()
