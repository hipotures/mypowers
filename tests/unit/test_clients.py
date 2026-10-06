import asyncio
import json
from unittest.mock import AsyncMock

import httpx
import pytest

from mypowers.client import Client
from mypowers.config import ClientConfig
from mypowers.contracts import AppError, Command, Output, StreamMessage


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


@pytest.mark.parametrize(
    ("fragmented", "size"),
    [(False, None), (True, None), (False, 32768), (True, 32768), (False, 32769), (True, 32769)],
    ids=["record", "record-fragmented", "limit", "limit-fragmented", "over", "over-fragmented"],
)
async def test_shared_log_stream_preserves_bounded_record_envelopes(core, fragmented, size):
    from websockets.asyncio.server import serve
    from websockets.exceptions import ConnectionClosedError

    snapshot = core[0].snapshot()
    record = {
        "schema_version": 1,
        "timestamp": snapshot.server_time,
        "server_instance_id": str(snapshot.server_instance_id),
        "sequence": 1,
        "level": "CRITICAL",
        "logger": "mypowers",
        "event": "bounded_transport",
        "message": "x" * 4096,
        "context": {f"key{i}": "y" * 3960 for i in range(3)},
    }
    initial = StreamMessage(
        type="snapshot",
        server_instance_id=snapshot.server_instance_id,
        stream_sequence=1,
        server_time=snapshot.server_time,
        data=snapshot.model_dump(mode="json"),
    )
    update = initial.model_copy(update={"type": "log", "stream_sequence": 2, "data": record})
    payload = json.dumps(update.model_dump(mode="json"), separators=(",", ":"))
    assert len(json.dumps(record, separators=(",", ":"))) + 1 <= 16384
    assert 16384 < len(payload) <= 32768
    if size is not None:
        record["context"]["padding"] = ""
        empty = json.dumps(update.model_dump(mode="json"), separators=(",", ":"))
        record["context"]["padding"] = "x" * (size - len(empty))
        payload = json.dumps(update.model_dump(mode="json"), separators=(",", ":"))
        assert len(payload) == size

    async def handler(socket):
        assert socket.request.path == "/api/v1/logs/stream?min_level=ERROR"
        await socket.send(initial.model_dump_json())
        if fragmented:
            middle = len(payload) // 2
            await socket.send([payload[:middle], payload[middle:]])
        else:
            await socket.send(payload)
        await socket.wait_closed()

    async with serve(handler, "127.0.0.1", 0, compression=None) as server:
        port = server.sockets[0].getsockname()[1]
        async with Client(ClientConfig(server=f"http://127.0.0.1:{port}")) as client:
            stream = client.stream(logs=True, min_level="ERROR")
            try:
                assert (await asyncio.wait_for(anext(stream), 2)).type == "snapshot"
                if size == 32769:
                    with pytest.raises(ConnectionClosedError) as caught:
                        await asyncio.wait_for(anext(stream), 2)
                    assert caught.value.sent.code == 1009
                else:
                    received = await asyncio.wait_for(anext(stream), 2)
                    assert received.type == "log" and received.data == record
            finally:
                await stream.aclose()


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
