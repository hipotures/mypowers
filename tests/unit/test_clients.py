import json
from unittest.mock import AsyncMock

import httpx
import pytest
from mypowers_cli.main import command_exit, duration, parser, query_time, run

from mypowers.client import Client
from mypowers.config import ClientConfig
from mypowers.contracts import AppError, Command, Output


@pytest.mark.parametrize(
    "args",
    [
        ["status", "--json"],
        ["--json", "status"],
        ["--env-file", ".env", "status"],
        ["--server", "https://localhost", "status"],
        ["ac", "on"],
        ["dc", "off"],
        ["light", "on"],
        ["command", "abc", "--json"],
        ["history", "--since", "1h", "--limit", "100"],
        ["logs", "--tail", "10"],
        ["logs", "--since", "30m", "--level", "WARNING"],
        ["logs", "--follow", "--json"],
        ["debug", "on", "--duration", "15m"],
        ["debug", "off"],
        ["connection", "pause"],
        ["connection", "resume"],
        ["connection", "retry"],
        ["tui"],
    ],
)
def test_documented_cli_arguments(args):
    assert parser().parse_args(args).command


@pytest.mark.parametrize(
    "args",
    [
        ["status", "--json"],
        ["status", "--no-color"],
        ["capabilities", "--json"],
        ["ac", "on", "--json"],
        ["dc", "off"],
        ["light", "on"],
        ["command", "known", "--json"],
        ["history", "--since", "1h", "--json"],
        ["history", "--since", "2026-10-04T00:00:00Z"],
        ["logs", "--tail", "10"],
        ["logs", "--follow", "--json"],
        ["logs", "--since", "30m", "--level", "WARNING"],
        ["debug", "on"],
        ["debug", "on", "--duration", "15m"],
        ["debug", "off"],
        ["connection", "pause"],
        ["connection", "resume"],
        ["connection", "retry"],
    ],
)
async def test_each_cli_command_clean_output_and_request(core, monkeypatch, capsys, args):
    import mypowers_cli.main as cli

    config = ClientConfig(no_color=True, timezone="Europe/Warsaw")
    command = Command(
        command_id="known",
        status="confirmed",
        output=Output.AC,
        requested_enabled=True,
        created_at=core[0].snapshot().server_time,
    )
    requests = []

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            pass

        async def status(self):
            return core[0].snapshot()

        async def admit(self, output, enabled):
            requests.append(("admit", output, enabled))
            return command

        async def wait_command(self, value):
            return value

        async def command(self, value):
            return command

        async def request(self, method, path, **kwargs):
            requests.append((method, path, kwargs))
            if path == "/logs":
                return {
                    "items": [
                        {
                            "timestamp": core[0].snapshot().server_time,
                            "level": "INFO",
                            "event": "event",
                            "message": "hello",
                        }
                    ]
                }
            if path == "/history":
                return {
                    "items": [
                        {
                            "received_at_ms": 1791130000000,
                            "battery_percent": 88,
                            "input_power_w": 0,
                            "output_power_w": 0,
                            "segment_id": "segment",
                        }
                    ],
                    "next_cursor": "next",
                }
            return {"state": "ok"}

        async def stream(self, **kwargs):
            from mypowers.contracts import StreamMessage

            yield StreamMessage(
                type="log",
                server_instance_id="instance",
                stream_sequence=1,
                server_time=core[0].snapshot().server_time,
                data={
                    "timestamp": core[0].snapshot().server_time,
                    "level": "INFO",
                    "event": "test",
                    "message": "followed",
                },
            )

    monkeypatch.setattr(cli, "Client", lambda _: FakeClient())
    parsed = parser().parse_args(args)
    assert await run(parsed, config) == 0
    output = capsys.readouterr()
    assert not output.err and "\x1b" not in output.out
    if getattr(parsed, "json", False):
        for line in output.out.splitlines():
            assert isinstance(json.loads(line), dict)
    if parsed.command in {"ac", "dc", "light"}:
        assert len(requests) == 1  # no repeated physical intention while waiting


@pytest.mark.parametrize(
    "method,path,status,body",
    [
        ("GET", "/status", 200, []),
        ("GET", "/status", 302, {}),
        ("GET", "/status", 401, {"error": {"code": "unauthorized", "message": "denied"}}),
        ("GET", "/status", 200, {"not": "status"}),
    ],
)
async def test_shared_client_invalid_auth_redirect_schema(method, path, status, body):
    async with Client(ClientConfig()) as client:
        await client.http.aclose()
        client.http = httpx.AsyncClient(
            base_url="http://localhost",
            transport=httpx.MockTransport(lambda request: httpx.Response(status, json=body)),
        )
        with pytest.raises(AppError):
            if body == {"not": "status"}:
                await client.status()
            else:
                await client.request(method, path)


async def test_client_uncertain_admission_never_replays(core):
    async with Client(ClientConfig()) as client:
        client.status = AsyncMock(return_value=core[0].snapshot())
        client.request = AsyncMock(side_effect=AppError("server_unreachable", "lost", 0))
        with pytest.raises(AppError) as caught:
            await client.admit(Output.AC, True)
        assert caught.value.code == "admission_unknown"
        assert client.request.await_count == 1
        command = Command(
            command_id="known", output=Output.AC, requested_enabled=True, created_at="now"
        )
        result = await client.wait_command(command, budget=0.01)
        assert result.status == "unconfirmed"
        assert (
            result.reason_code == "client_wait_expired"
            or result.reason_code == "client_connection_lost"
        )
        assert command_exit(result.model_dump()) == 6


def test_times_zero_outcome_exit_codes():
    config = ClientConfig(timezone="Europe/Warsaw")
    assert duration("15m") == 900
    assert query_time("1h", "2026-10-04T12:00:00Z", config) == "2026-10-04T11:00:00Z"
    assert query_time("2026-10-04T12:00:00", "", config) == "2026-10-04T10:00:00Z"
    with pytest.raises(ValueError):
        query_time("2026-10-04T12:00:00", "", ClientConfig())
    for bad in ("0s", "2d", "wrong"):
        with pytest.raises(ValueError):
            duration(bad)
    assert command_exit({"status": "rejected"}) == 5
    assert command_exit({"status": "failed"}) == 1
