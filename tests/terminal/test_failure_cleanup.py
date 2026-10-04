import asyncio
import os
import pty
import select
import sys
import termios
import time

import pytest
from mypowers_tui.dashboard import Dashboard
from mypowers_tui.main import Application
from mypowers_tui.terminal import RESTORE, Terminal

from mypowers.config import ClientConfig
from mypowers.contracts import Command, Output, StreamMessage


@pytest.mark.parametrize("exception", [False, True])
def test_terminal_modes_restore_after_render_error_and_initialization(monkeypatch, exception):
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    stdin = os.fdopen(os.dup(slave), "r")
    stdout = os.fdopen(os.dup(slave), "w")
    monkeypatch.setattr(sys, "stdin", stdin)
    monkeypatch.setattr(sys, "stdout", stdout)
    try:
        if exception:
            with pytest.raises(RuntimeError):
                with Terminal():
                    assert termios.tcgetattr(slave) != original
                    raise RuntimeError("injected render failure")
        else:
            with Terminal(mouse=False):
                assert termios.tcgetattr(slave) != original
        assert termios.tcgetattr(slave) == original
        captured = bytearray()
        deadline = time.monotonic() + 2
        while RESTORE.encode() not in captured and time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                captured.extend(os.read(master, 4096))
        assert RESTORE.encode() in captured
    finally:
        stdin.close()
        stdout.close()
        os.close(master)
        os.close(slave)


async def test_tui_server_failure_runtime_actions_and_unconfirmed_display(core):
    dashboard = Dashboard()
    dashboard.update(core[0].snapshot())
    stop = asyncio.Event()
    calls = []

    class Client:
        async def request(self, method, path, **kwargs):
            calls.append((method, path, kwargs))
            return {}

        async def admit(self, output, enabled, snapshot):
            return Command(
                command_id="known", output=output, requested_enabled=enabled, created_at="now"
            )

        async def wait_command(self, value):
            return value.model_copy(update={"status": "unconfirmed"})

        async def stream(self, logs=False, **kwargs):
            if logs:
                yield StreamMessage(
                    type="gap", server_instance_id="instance", stream_sequence=1, server_time="now"
                )
                yield StreamMessage(
                    type="log",
                    server_instance_id="instance",
                    stream_sequence=2,
                    server_time="now",
                    data={"level": "INFO", "message": "hello"},
                )
            else:
                yield StreamMessage(
                    type="snapshot",
                    server_instance_id="instance",
                    stream_sequence=1,
                    server_time="now",
                    data=core[0].snapshot().model_dump(mode="json"),
                )
                yield StreamMessage(
                    type="command",
                    server_instance_id="instance",
                    stream_sequence=2,
                    server_time="now",
                    data={"output": "ac", "status": "unconfirmed"},
                )
            stop.set()
            raise OSError("stream lost")

    app = Application(Client(), dashboard, stop)
    await app.state_stream()
    assert not dashboard.server_connected
    stop.clear()
    await app.log_stream()
    assert len(dashboard.logs) == 1
    dashboard.server_connected = True
    for action in ("retry", "pause", "debug"):
        await app.operation(action)
    dashboard.status = dashboard.status.model_copy(update={"logging": {"effective_level": "DEBUG"}})
    await app.operation("debug")
    assert calls[-1][0] == "DELETE"
    await app.control(Output.AC, True, dashboard.status)
    assert "uncertain" in dashboard.notice and "known" in dashboard.notice
    app.activate("help")
    dashboard.render(100, 28)
    app.activate("logs")
    dashboard.render(80, 24)
    app.activate("quit")
    assert stop.is_set()
    await app.close()


async def test_tui_actual_render_loop_api_failure_restores_terminal(monkeypatch):
    import mypowers_tui.main as tui

    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    stdin = os.fdopen(os.dup(slave), "r")
    stdout = os.fdopen(os.dup(slave), "w")
    monkeypatch.setattr(sys, "stdin", stdin)
    monkeypatch.setattr(sys, "stdout", stdout)

    class Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            pass

        async def stream(self, **kwargs):
            raise OSError("fake unavailable server")
            yield  # async generator interface

    monkeypatch.setattr(tui, "Client", lambda _: Client())
    loop = asyncio.get_running_loop()
    loop.call_later(0.1, os.write, master, b"q")
    try:
        await tui.run(ClientConfig())
        assert termios.tcgetattr(slave) == original
    finally:
        stdin.close()
        stdout.close()
        os.close(master)
        os.close(slave)
