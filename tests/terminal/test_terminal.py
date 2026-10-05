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
import struct
import subprocess
import termios
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path

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
