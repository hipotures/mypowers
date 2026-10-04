"""Exercise daemon startup with the same colored stderr path as an interactive shell."""

import os
import pty
import re
import select
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path


def test_daemon_startup_logs_in_colored_terminal(tmp_path):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    selected = tmp_path / "simulated.yaml"
    selected.write_text(
        "environment: development\nbackend: simulated\napi:\n  auth_required: false\n"
    )
    (tmp_path / ".env").write_text(
        f"MYPOWERS_CONFIG={selected}\nMYPOWERS_PORT={port}\n"
        "MYPOWERS_DATA_DIR=./data\nMYPOWERS_LOG_DIR=./logs\nMYPOWERS_RUNTIME_DIR=./run\n"
    )
    env = {**os.environ, "TERM": "xterm-256color"}
    master, slave = pty.openpty()
    process = subprocess.Popen(
        [str(Path(sys.executable).parent / "mypowersd")],
        cwd=tmp_path,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
    )
    captured = bytearray()
    expected = f"Uvicorn running on http://127.0.0.1:{port}"
    try:
        deadline = time.monotonic() + 10
        clean = ""
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                captured.extend(os.read(master, 65536))
            clean = re.sub(r"\x1b\[[0-9;]*m", "", captured.decode(errors="replace"))
            if expected in clean:
                break
            assert process.poll() is None, clean
        assert expected in clean
        assert f"Started server process [{process.pid}]" in clean
        assert "%d" not in clean and "%s" not in clean
        assert b"\x1b[" in captured, "The colored formatter must actually be enabled."
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise
        finally:
            os.close(master)
            os.close(slave)
