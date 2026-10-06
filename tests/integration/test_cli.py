import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


def native_cli():
    binary = Path(__file__).resolve().parents[2] / "frontends/tui/target/debug/mypowers"
    assert binary.is_file(), (
        "Build first: cargo build --locked --manifest-path frontends/tui/Cargo.toml"
    )
    return binary


@pytest.mark.parametrize(
    "command",
    [
        ["status", "--json"],
        ["status", "--require-live"],
        ["capabilities", "--json"],
        ["ac", "on", "--json"],
        ["dc", "on", "--json"],
        ["light", "on", "--json"],
        ["history", "--since", "1h", "--json"],
        ["logs", "--tail", "10", "--json"],
        ["debug", "on", "--duration", "15m", "--json"],
        ["debug", "off", "--json"],
        ["connection", "retry", "--json"],
    ],
)
def test_installed_cli_each_real_request(daemon_process, command):
    _, url, env = daemon_process
    executable = native_cli()
    result = subprocess.run(
        [str(executable), "--server", url, *command],
        env=env,
        capture_output=True,
        text=True,
        timeout=8,
    )
    assert result.returncode == 0, result.stderr + result.stdout
    assert "\x1b" not in result.stdout
    if "--json" in command:
        data = json.loads(result.stdout)
        if command[0] in {"ac", "dc", "light"}:
            assert data["status"] == "confirmed"
        assert not result.stderr


def test_unreachable_cli_and_token_command(tmp_path):
    bindir = Path(sys.executable).parent
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYPOWERS_")}
    result = subprocess.run(
        [str(native_cli()), "--server", "http://127.0.0.1:1", "status", "--json"],
        env=env,
        capture_output=True,
        text=True,
        timeout=3,
    )
    assert result.returncode == 3
    assert json.loads(result.stdout)["error"]["code"] == "server_unreachable"
    token = tmp_path / "api-token"
    result = subprocess.run(
        [str(bindir / "mypowersd"), "token", "generate", "--output", str(token)],
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0 and token.stat().st_mode & 0o777 == 0o600
    assert len(token.read_text().strip()) >= 32 and token.read_text().strip() not in result.stdout
    again = subprocess.run(
        [str(bindir / "mypowersd"), "token", "generate", "--output", str(token)],
        env=env,
        capture_output=True,
        text=True,
    )
    assert again.returncode == 2


def test_installed_cli_reads_current_dotenv_without_option(daemon_process, tmp_path):
    _, url, env = daemon_process
    (tmp_path / ".env").write_text(f"MYPOWERS_SERVER_URL={url}\n")
    env = {key: value for key, value in env.items() if not key.startswith("MYPOWERS_")}
    result = subprocess.run(
        [str(native_cli()), "status", "--json"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=8,
    )
    assert result.returncode == 0, result.stderr + result.stdout
    assert json.loads(result.stdout)["server"]["backend"] == "simulated"


def test_native_snapshot_is_read_only_and_json_does_not_fetch_history(core, tmp_path):
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from urllib.parse import parse_qs, urlsplit

    from mypowers.contracts import Settings

    snapshot = core[0].snapshot().model_dump(mode="json")
    snapshot["history"]["state"] = "ok"
    calls = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            parsed = urlsplit(self.path)
            calls.append(parsed.path)
            if parsed.path.endswith("/status"):
                data = snapshot
            elif parsed.path.endswith("/settings"):
                data = Settings(graph_visualization="chart", graph_interval_seconds=30).model_dump()
            elif parsed.path.endswith("/history/aggregates"):
                query = parse_qs(parsed.query)
                from datetime import datetime

                data = {
                    "schema_version": 1,
                    "source": "database",
                    "bucket_seconds": int(query["bucket_seconds"][0]),
                    "since_ms": round(datetime.fromisoformat(query["since"][0]).timestamp() * 1000),
                    "until_ms": round(datetime.fromisoformat(query["until"][0]).timestamp() * 1000),
                    "items": [],
                }
            else:
                raise AssertionError(self.path)
            body = json.dumps(data).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYPOWERS_")}
    try:
        command = [
            str(native_cli()),
            "--server",
            f"http://127.0.0.1:{server.server_port}",
            "status",
        ]
        result = subprocess.run(command, cwd=tmp_path, env=env, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        assert "MYPOWERS" in result.stdout and "OFF" in result.stdout
        assert "q quit" not in result.stdout and "\x1b" not in result.stdout
        assert calls == ["/api/v1/status", "/api/v1/settings", "/api/v1/history/aggregates"]
        calls.clear()
        result = subprocess.run(
            [*command, "--json"], cwd=tmp_path, env=env, capture_output=True, text=True
        )
        assert result.returncode == 0 and json.loads(result.stdout) == snapshot
        assert calls == ["/api/v1/status"]
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@pytest.mark.parametrize("mode", ["lost_admission", "invalid_admission", "changed_poll"])
def test_native_output_never_replays_uncertain_or_changed_receipts(core, tmp_path, mode):
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    snapshot = core[0].snapshot().model_dump(mode="json")
    calls = []
    receipt = {
        "schema_version": 1,
        "command_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
        "status": "accepted",
        "output": "ac",
        "requested_enabled": True,
        "reason_code": None,
    }

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, data):
            body = json.dumps(data).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            calls.append(("GET", self.path))
            self.reply(
                snapshot
                if self.path == "/api/v1/status"
                else {**receipt, "output": "dc", "status": "confirmed"}
            )

        def do_PUT(self):
            calls.append(("PUT", self.path))
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            assert body == {
                "enabled": True,
                "server_instance_id": snapshot["server_instance_id"],
                "expected_outputs_revision": snapshot["controls"]["outputs_revision"],
            }
            assert self.headers["Idempotency-Key"]
            if mode == "lost_admission":
                self.close_connection = True
            else:
                self.reply({} if mode == "invalid_admission" else receipt)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYPOWERS_")}
    try:
        result = subprocess.run(
            [
                str(native_cli()),
                "--server",
                f"http://127.0.0.1:{server.server_port}",
                "ac",
                "on",
                "--json",
            ],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=5,
        )
        assert result.returncode == 6, result.stdout + result.stderr
        assert calls.count(("PUT", "/api/v1/outputs/ac")) == 1
        data = json.loads(result.stdout)
        if mode == "changed_poll":
            assert data["status"] == "unconfirmed" and data["output"] == "ac"
        else:
            assert data["error"]["code"] == "admission_unknown"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_native_log_follow_keeps_json_records_and_stops_on_ctrl_c(daemon_process, tmp_path):
    import select
    import signal
    import time

    import httpx

    _, url, env = daemon_process
    process = subprocess.Popen(
        [str(native_cli()), "--server", url, "logs", "--follow", "--level", "INFO", "--json"],
        cwd=tmp_path,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 5
        captured = b""
        with httpx.Client(trust_env=False) as client:
            while time.monotonic() < deadline:
                # Repeated test-only log-level updates make stream startup timing irrelevant.
                client.put(url + "/api/v1/runtime/log-level", json={"level": "DEBUG"})
                if select.select([process.stdout], [], [], 0.1)[0]:
                    captured += os.read(process.stdout.fileno(), 65536)
                    lines = captured.rsplit(b"\n", 1)[0].splitlines()
                    if any(b'"event":"log_level_changed"' in line for line in lines[1:]):
                        break
        lines = [json.loads(line) for line in captured.rsplit(b"\n", 1)[0].splitlines()]
        assert lines and "items" in lines[0]
        assert any(line.get("event") == "log_level_changed" for line in lines[1:]), captured
        assert b"\x1b" not in captured
        process.send_signal(signal.SIGINT)
        _, stderr = process.communicate(timeout=3)
        assert process.returncode == 130 and not stderr
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


@pytest.mark.parametrize("daemon_process", [True], indirect=True)
def test_native_auth_error_retains_exit_code_and_clean_json(daemon_process, tmp_path):
    _, url, env = daemon_process
    env = {key: value for key, value in env.items() if key != "MYPOWERS_API_TOKEN"}
    result = subprocess.run(
        [str(native_cli()), "--server", url, "status", "--json"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=3,
    )
    assert result.returncode == 4 and not result.stderr
    assert json.loads(result.stdout)["error"]["code"] == "unauthorized"
    assert "a" * 40 not in result.stdout


@pytest.mark.parametrize("options", [["--ca-file", "missing-ca.pem"], ["--timeout", "0"]])
def test_native_configuration_errors_keep_usage_exit_code(tmp_path, options):
    env = {key: value for key, value in os.environ.items() if not key.startswith("MYPOWERS_")}
    result = subprocess.run(
        [str(native_cli()), *options, "status", "--json"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=3,
    )
    assert result.returncode == 2
