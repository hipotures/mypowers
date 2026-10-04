"""Run with an explicitly provisioned Caddy binary; no system CA installation."""

import asyncio
import json
import os
import socket
import subprocess
import time
from pathlib import Path

import httpx
import pytest

from mypowers.client import Client
from mypowers.config import ClientConfig

CADDY_BINARY = os.environ.get("MYPOWERS_CADDY_TEST_BINARY")
TLS_RESULT_FILE = os.environ.get("MYPOWERS_TLS_RESULT_FILE")


async def test_caddy_https_wss_private_ca_and_auth(daemon_process, tmp_path):
    binary = CADDY_BINARY
    if binary is None:
        pytest.skip("Set MYPOWERS_CADDY_TEST_BINARY for the real local Caddy TLS integration.")
    _, url, env = daemon_process
    backend_port = url.rsplit(":", 1)[1]
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    # This environment is for Caddy only; its CA state remains in the test directory.
    env = {
        **env,
        "XDG_DATA_HOME": str(tmp_path / "caddy-data"),
        "XDG_CONFIG_HOME": str(tmp_path / "caddy-config"),
        "MYPOWERS_TEST_TLS_PORT": str(port),
        "MYPOWERS_TEST_BACKEND_PORT": backend_port,
    }
    config = Path(__file__).parents[2] / "deploy/caddy/Caddyfile.local-test"
    output = (tmp_path / "caddy.stderr").open("w")
    process = await asyncio.to_thread(
        subprocess.Popen,
        [binary, "run", "--config", str(config), "--adapter", "caddyfile"],
        env=env,
        stderr=output,
        stdout=subprocess.DEVNULL,
    )
    ca = tmp_path / "caddy-data/caddy/pki/authorities/local/root.crt"
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if process.poll() is not None:
                pytest.fail((tmp_path / "caddy.stderr").read_text())
            if ca.exists():
                try:
                    async with Client(
                        ClientConfig(server=f"https://localhost:{port}", ca_file=str(ca))
                    ) as client:
                        snapshot = await client.status()
                        assert snapshot.telemetry.state == "live"
                        stream = client.stream()
                        assert (await anext(stream)).type == "snapshot"
                        await stream.aclose()
                    break
                except Exception:
                    await asyncio.sleep(0.1)
            else:
                await asyncio.sleep(0.1)
        else:
            pytest.fail("Caddy TLS proxy did not become ready.")
        with pytest.raises(httpx.TransportError):
            async with httpx.AsyncClient(trust_env=False) as untrusted:
                await untrusted.get(f"https://localhost:{port}/health/live")
        artifacts = {
            "version": (
                await asyncio.to_thread(subprocess.check_output, [binary, "version"], text=True)
            ).strip(),
            "https": "PASS",
            "wss": "PASS",
            "untrusted_ca_rejected": "PASS",
            "system_trust_modified": False,
        }
        result = TLS_RESULT_FILE
        if result:
            await asyncio.to_thread(Path(result).write_text, json.dumps(artifacts, indent=2) + "\n")
    finally:
        process.terminate()
        await asyncio.to_thread(process.wait, timeout=5)
        output.close()
