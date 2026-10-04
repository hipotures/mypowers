"""All default tests use explicit fakes and isolated paths, never local dotenv/BLE."""

import asyncio
import os
import subprocess
import sys
import time
from functools import reduce
from operator import xor
from pathlib import Path
from uuid import UUID

import httpx
import pytest

from mypowers.config.server import API, ServerConfig
from mypowers.contracts import Output, OutputRequest
from mypowers.core import Core
from mypowers.protocol import PROFILE, STATION_ADDRESS, STATION_NAME


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch):
    monkeypatch.delenv("WEB_CONCURRENCY", raising=False)
    for key in os.environ:
        if key.startswith("MYPOWERS_"):
            monkeypatch.delenv(key)


def frame(flags=12, battery=88, input_w=0, output_w=0, minutes=0):
    body = bytes.fromhex("a565b100010801") + bytes([flags, battery])
    body += input_w.to_bytes(2, "big") + output_w.to_bytes(2, "big") + minutes.to_bytes(2, "big")
    return body + bytes([reduce(xor, body, 0)])


class FakeClock:
    def __init__(self):
        self.mono = 100.0
        self.epoch = 1791130000.0

    def monotonic(self):
        return self.mono

    def time(self):
        return self.epoch

    def advance(self, value):
        self.mono += value
        self.epoch += value


class FakeWriter:
    def __init__(self):
        self.frames = []
        self.action = None

    async def write(self, value):
        self.frames.append(value)
        if self.action:
            await self.action(value)


@pytest.fixture
def core():
    clock = FakeClock()
    core = Core(
        address=STATION_ADDRESS,
        name=STATION_NAME,
        profile=PROFILE,
        adapter="F4:4E:FC:A1:CB:FF",
        clock=clock,
    )
    writer = FakeWriter()
    session = core.begin_session(writer, "hci7")
    core.receive(frame(), session)
    clock.advance(0.1)
    core.receive(frame(), session)
    return core, clock, writer, session


def request(core, enabled=True):
    return OutputRequest(
        enabled=enabled,
        server_instance_id=UUID(core.instance),
        expected_outputs_revision=core.revision,
    )


async def admit_and_fresh(bundle, output=Output.AC, enabled=True):
    core, clock, writer, session = bundle
    command = core.admit(output, request(core, enabled), "key")
    await asyncio.sleep(0)
    clock.advance(0.1)
    core.receive(frame(core.latest.sample.status_flags), session)
    await asyncio.sleep(0)
    return command


@pytest.fixture
def config(tmp_path):
    return ServerConfig(
        api=API(auth_required=False),
        backend="simulated",
        data_dir=tmp_path / "data",
        log_dir=tmp_path / "logs",
        runtime_dir=tmp_path / "run",
    )


@pytest.fixture
def daemon_process(tmp_path):
    """Real Uvicorn process, always simulated and environment-isolated."""
    import socket

    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = {key: value for key, value in os.environ.items() if not key.startswith("MYPOWERS_")}
    selected = tmp_path / "offline.yaml"
    selected.write_text("api:\n  auth_required: false\nhistory:\n  interval_seconds: 0.2\n")
    env.update(
        MYPOWERS_CONFIG=str(selected),
        MYPOWERS_AUTH_REQUIRED="false",
        MYPOWERS_BACKEND="simulated",
        MYPOWERS_DATA_DIR=str(tmp_path / "data"),
        MYPOWERS_LOG_DIR=str(tmp_path / "logs"),
        MYPOWERS_RUNTIME_DIR=str(tmp_path / "run"),
        MYPOWERS_PORT=str(port),
    )
    stderr = (tmp_path / "daemon.stderr").open("w")
    executable = Path(sys.executable).parent / "mypowersd"
    process = subprocess.Popen([str(executable)], env=env, stdout=subprocess.DEVNULL, stderr=stderr)
    url = f"http://127.0.0.1:{port}"
    try:
        with httpx.Client(trust_env=False) as client:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    pytest.fail((tmp_path / "daemon.stderr").read_text())
                try:
                    response = client.get(url + "/api/v1/status")
                    if response.status_code == 200 and response.json()["controls"]["allowed"]:
                        break
                except httpx.HTTPError:
                    pass
                time.sleep(0.05)
            else:
                pytest.fail("Simulated daemon did not become ready.")
        yield process, url, env
    finally:
        process.terminate()
        try:
            process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        stderr.close()
