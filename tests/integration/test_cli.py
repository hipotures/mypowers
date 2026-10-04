import json
import os
import subprocess
import sys
from pathlib import Path

import pytest


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
    executable = Path(sys.executable).parent / "mypowers"
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


def test_unreachable_cli_non_tty_tui_and_token_command(tmp_path):
    bindir = Path(sys.executable).parent
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYPOWERS_")}
    result = subprocess.run(
        [str(bindir / "mypowers"), "--server", "http://127.0.0.1:1", "status", "--json"],
        env=env,
        capture_output=True,
        text=True,
        timeout=3,
    )
    assert result.returncode == 3
    assert json.loads(result.stdout)["error"]["code"] == "server_unreachable"
    result = subprocess.run([str(bindir / "mypowers-tui")], env=env, capture_output=True, text=True)
    assert result.returncode == 2 and "terminal" in result.stderr.lower()
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
        [str(Path(sys.executable).parent / "mypowers"), "status", "--json"],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=8,
    )
    assert result.returncode == 0, result.stderr + result.stdout
    assert json.loads(result.stdout)["server"]["backend"] == "simulated"
