"""Test the production terminal guard without a daemon or desktop."""

import fcntl
import os
import pty
import subprocess
import termios

import pytest
from test_terminal import ROOT, Session


@pytest.fixture(scope="module")
def cleanup_binary():
    subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--manifest-path",
            str(ROOT / "frontends/tui/Cargo.toml"),
            "--example",
            "terminal_cleanup",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return ROOT / "frontends/tui/target/debug/examples/terminal_cleanup"


@pytest.mark.parametrize(
    ("mode", "exit_code"),
    [("normal", 0), ("main-panic", 101), ("worker-panic", 101), ("task-panic", 101)],
)
def test_guard_restores_terminal_and_panics_stop_the_process(
    cleanup_binary, tmp_path, mode, exit_code
):
    session = Session(cleanup_binary, tmp_path, os.environ.copy(), mode)
    try:
        captured = session.read(b"\x1b[?25h")
        assert b"SESSION READY" in captured
        assert b"\x1b[?1006l" in captured
        assert b"\x1b[?1049l" in captured
        assert b"\x1b[?25h" in captured
        assert session.process.wait(timeout=3) == exit_code
        assert termios.tcgetattr(session.slave) == session.original
        assert b"WORKER PANIC SURVIVED" not in captured
        assert b"TASK PANIC SURVIVED" not in captured
    finally:
        session.close()


def test_crossterm_consumes_the_entire_input_burst(cleanup_binary, tmp_path):
    session = Session(cleanup_binary, tmp_path, os.environ.copy(), "input-burst")
    try:
        session.read(b"SESSION READY")
        payload = b"\t" * 2048 + b"\x11"
        assert os.write(session.master, payload) == len(payload)
        captured = session.read(b"\x1b[?25h", budget=2)
        assert b"INPUT COMPLETE 2048" in captured
        assert session.process.wait(timeout=2) == 0
        assert termios.tcgetattr(session.slave) == session.original
    finally:
        session.close()


def test_setup_write_failure_restores_raw_mode(cleanup_binary, tmp_path):
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    reader, writer = os.pipe()
    os.close(reader)
    process = None

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    try:
        process = subprocess.Popen(
            [str(cleanup_binary), "normal"],
            cwd=tmp_path,
            stdin=slave,
            stdout=writer,
            stderr=slave,
            preexec_fn=controlling_terminal,
        )
        assert process.wait(timeout=3) != 0
        assert termios.tcgetattr(slave) == original
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        os.close(writer)
        os.close(master)
        os.close(slave)
