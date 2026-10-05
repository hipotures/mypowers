import asyncio
import json
import logging
import time
from uuid import uuid4

import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

from mypowers.api import create_app
from mypowers.contracts import Status, StreamMessage
from mypowers.daemon.service import Service


def ready(client):
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        data = client.get("/api/v1/status").json()
        if data["controls"]["allowed"]:
            return data
        time.sleep(0.02)
    pytest.fail("Fake telemetry not live")


def test_full_http_lifecycle_contract_commands_history_debug(config):
    with TestClient(create_app(config)) as client:
        assert client.get("/health/live").json() == {"status": "ok"}
        snapshot = ready(client)
        Status.model_validate(snapshot)
        assert client.get("/api/v1/status").headers["cache-control"] == "no-store"
        capabilities = client.get("/api/v1/capabilities").json()
        assert capabilities["outputs"] == ["ac", "dc", "light"]
        body = {
            "enabled": True,
            "server_instance_id": snapshot["server_instance_id"],
            "expected_outputs_revision": snapshot["controls"]["outputs_revision"],
        }
        key = str(uuid4())
        response = client.put("/api/v1/outputs/ac", json=body, headers={"Idempotency-Key": key})
        assert response.status_code == 202
        command = response.json()
        command_id = command["command_id"]
        duplicate = client.put("/api/v1/outputs/ac", json=body, headers={"Idempotency-Key": key})
        assert duplicate.json()["command_id"] == command_id
        assert (
            client.put(
                "/api/v1/outputs/dc", json=body, headers={"Idempotency-Key": str(uuid4())}
            ).status_code
            == 409
        )
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            command = client.get(f"/api/v1/commands/{command_id}").json()
            if command["status"] == "confirmed":
                break
            time.sleep(0.02)
        assert command["status"] == "confirmed"
        assert len(command["confirmation_sequences"]) == 2
        assert client.get("/api/v1/history").json()["items"]
        assert (
            client.put(
                "/api/v1/runtime/log-level", json={"level": "DEBUG", "duration_seconds": 0.1}
            ).json()["effective_level"]
            == "DEBUG"
        )
        time.sleep(0.15)
        assert client.get("/api/v1/status").json()["logging"]["effective_level"] == "INFO"
        assert client.delete("/api/v1/runtime/log-level").status_code == 200
        assert client.get("/api/v1/logs?tail=10").json()["items"]
        assert client.put("/api/v1/connection", json={"desired": "paused"}).status_code == 200
        time.sleep(0.1)
        assert client.get("/api/v1/status").json()["connection"]["desired"] == "paused"
        assert client.put("/api/v1/connection", json={"desired": "running"}).status_code == 200
        assert client.post("/api/v1/connection/retry", json={}).status_code == 200
        assert client.get(f"/api/v1/commands/{uuid4()}").status_code == 404


@pytest.mark.parametrize(
    "body",
    [
        {"enabled": "true"},
        {"enabled": 1},
        {"enabled": True, "extra": 1},
        {"enabled": False, "expected_outputs_revision": "1"},
    ],
)
def test_strict_booleans_unknown_fields_and_error_envelope(config, body):
    with TestClient(create_app(config)) as client:
        response = client.put(
            "/api/v1/outputs/ac", json=body, headers={"Idempotency-Key": str(uuid4())}
        )
        assert response.status_code == 422
        assert response.json()["error"]["code"] == "invalid_request"
        assert "input" not in response.text
        assert client.get("/api/v1/history?limit=10001").status_code == 422
        assert client.get("/api/v1/history?since=2026-10-04T00:00:00").status_code in {422, 503}
        assert (
            client.put(
                "/api/v1/outputs/raw", json={}, headers={"Idempotency-Key": str(uuid4())}
            ).status_code
            == 422
        )
        assert client.put("/api/v1/runtime/log-level", json={"level": "TRACE"}).status_code == 422


@pytest.mark.parametrize(
    "path,method",
    [
        ("/api/v1/status", "get"),
        ("/api/v1/capabilities", "get"),
        ("/api/v1/history", "get"),
        ("/api/v1/history/aggregates", "get"),
        ("/api/v1/logs", "get"),
        ("/api/v1/commands/" + str(uuid4()), "get"),
        ("/api/v1/outputs/ac", "put"),
        ("/api/v1/connection", "put"),
        ("/api/v1/connection/retry", "post"),
        ("/api/v1/runtime/log-level", "put"),
        ("/api/v1/runtime/log-level", "delete"),
    ],
)
def test_every_http_auth_boundary(config, path, method):
    secret = "safe-test-token-" * 3
    config.api.auth_required = True
    config.api_token = secret
    with TestClient(create_app(config)) as client:
        response = getattr(client, method)(path)
        assert response.status_code == 401
        assert secret not in response.text
        assert client.get("/health/live").json() == {"status": "ok"}
        assert (
            client.get("/api/v1/status", headers={"Authorization": "Bearer " + secret}).status_code
            == 200
        )


def test_body_limits_host_origin_content_type(config):
    with TestClient(create_app(config)) as client:
        assert (
            client.post(
                "/api/v1/connection/retry", content="{}", headers={"Content-Type": "text/plain"}
            ).status_code
            == 422
        )
        assert (
            client.post(
                "/api/v1/connection/retry",
                content=" " * 17000,
                headers={"Content-Type": "application/json"},
            ).status_code
            == 413
        )
        assert (
            client.post(
                "/api/v1/connection/retry",
                content=iter([b" " * 8000, b" " * 9000]),
                headers={"Content-Type": "application/json"},
            ).status_code
            == 413
        )
        assert client.get("/api/v1/status", headers={"Host": "attacker.example"}).status_code == 403
        assert (
            client.post(
                "/api/v1/connection/retry", json={}, headers={"Origin": "https://attacker.example"}
            ).status_code
            == 403
        )
        assert (
            client.post(
                "/api/v1/connection/retry", json={}, headers={"Origin": "http://localhost:8765"}
            ).status_code
            == 200
        )


def test_websocket_snapshot_heartbeat_and_auth(config):
    secret = "safe-test-token-" * 3
    config.api.auth_required = True
    config.api_token = secret
    with TestClient(create_app(config)) as client:
        with client.websocket_connect(
            "/api/v1/events", headers={"Authorization": "Bearer " + secret}
        ) as ws:
            first = ws.receive_json()
            assert first["type"] == "snapshot" and first["stream_sequence"] == 1
            second = ws.receive_json()
            assert second["stream_sequence"] > 1
        with client.websocket_connect("/api/v1/events") as ws:
            ws.send_json({"type": "authenticate", "token": secret})
            assert ws.receive_json()["type"] == "snapshot"
        with pytest.raises(WebSocketDisconnect):
            with client.websocket_connect("/api/v1/events") as ws:
                ws.send_json({"type": "authenticate", "token": "wrong"})
                ws.receive_json()
        with pytest.raises(WebSocketDisconnect):
            with client.websocket_connect("/api/v1/events?token=" + secret) as ws:
                ws.receive_json()
        with pytest.raises(WebSocketDisconnect):
            with client.websocket_connect(
                "/api/v1/events", headers={"Origin": "https://attacker.example"}
            ) as ws:
                ws.receive_json()
        with client.websocket_connect(
            "/api/v1/logs/stream", headers={"Authorization": "Bearer " + secret}
        ) as ws:
            assert ws.receive_json()["type"] == "snapshot"
            assert ws.receive_json()["type"] in {"log", "heartbeat"}


@pytest.mark.parametrize(
    ("message", "context", "truncated", "large_envelope"),
    [
        ("🔋" * 4096, {}, True, False),
        ("x" * 4096, {f"key{i}": "y" * 3960 for i in range(3)}, False, True),
        ("Station metric", {"payload": "電" * 4096}, True, False),
    ],
    ids=["unicode-message", "near-limit-context", "oversized-context"],
)
def test_bounded_logs_replay_and_arrive_live_through_the_actual_api(
    config, message, context, truncated, large_envelope
):
    service = Service(config)
    with TestClient(create_app(config, service)) as client:
        initial = ready(client)

        def emit(event):
            record = logging.LogRecord("mypowers", logging.CRITICAL, "", 0, message, (), None)
            record.event = event
            record.context = context
            service.logs.handler.emit(record)
            with service.logs.mutex:
                return next(row for row in reversed(service.logs.records) if row["event"] == event)

        async def flush_records():
            await asyncio.wait_for(asyncio.to_thread(service.logs.queue.join), 2)

        retained = emit("bounded_transport_retained")
        client.portal.call(flush_records)
        with client.websocket_connect("/api/v1/logs/stream?min_level=ERROR") as ws:
            snapshot = ws.receive_json()
            assert snapshot["type"] == "snapshot"
            raw = ws.receive_text()
            replay = json.loads(raw)
            StreamMessage.model_validate(replay)
            assert replay["type"] == "log" and replay["data"] == retained
            assert replay["stream_sequence"] > snapshot["stream_sequence"]
            assert len(raw.encode("utf-8")) <= 32768
            if large_envelope:
                assert len(raw.encode("utf-8")) > 16384

            current = emit("bounded_transport_live")
            raw = ws.receive_text()
            live = json.loads(raw)
            StreamMessage.model_validate(live)
            assert live["type"] == "log" and live["data"] == current
            assert live["stream_sequence"] > replay["stream_sequence"]
            assert len(raw.encode("utf-8")) <= 32768

        client.portal.call(flush_records)
        response = client.get("/api/v1/logs?tail=10&min_level=ERROR")
        assert response.status_code == 200
        page = response.json()
        assert page["source"] == "files" and page["skipped_lines"] == 0
        assert page["items"] == [retained, current]
        for row in page["items"]:
            assert bool(row["context"].get("truncated")) == truncated
            assert message.startswith(row["message"])
            if not truncated:
                assert row["message"] == message and row["context"] == context
        lines = config.log_dir.joinpath("mypowers.jsonl").read_bytes().splitlines(keepends=True)
        assert all(len(line) <= 16384 for line in lines)
        assert (
            client.get("/api/v1/status").json()["server_instance_id"]
            == initial["server_instance_id"]
        )


def test_unavailable_transport_http_startup_and_task_health(config):
    from mypowers.contracts import AppError

    class Unavailable:
        adapter_id = "missing"

        async def acquire(self, phase):
            await asyncio.sleep(0.01)
            raise AppError("adapter_missing", "Adapter missing.", 503)

        async def cleanup(self):
            pass

    service = Service(config, Unavailable)
    with TestClient(create_app(config, service)) as client:
        time.sleep(0.05)
        assert client.get("/health/live").status_code == 200
        data = client.get("/api/v1/status").json()
        assert not data["connection"]["link_connected"]
        assert data["telemetry"]["sample"] is None
        assert data["diagnostics"]["healthy"]


async def test_real_uvicorn_and_shared_client(daemon_process):
    from mypowers.client import Client
    from mypowers.config import ClientConfig

    _, url, _ = daemon_process
    async with Client(ClientConfig(server=url)) as client:
        snapshot = await client.status()
        assert snapshot.telemetry.state == "live"
        stream = client.stream()
        first = await anext(stream)
        assert first.type == "snapshot"
        await stream.aclose()
        command = await client.admit(
            __import__("mypowers.contracts", fromlist=["Output"]).Output.LIGHT, True
        )
        assert (await client.wait_command(command)).status == "confirmed"
        assert (await client.request("GET", "/history"))["items"]


def test_openapi_typed_error_responses_operation_ids(config):
    schema = create_app(config).openapi()
    operation_ids = []
    for path, routes in schema["paths"].items():
        for operation in routes.values():
            operation_ids.append(operation["operationId"])
            if path != "/health/live":
                assert operation["responses"]["422"]["content"]["application/json"]["schema"][
                    "$ref"
                ].endswith("ErrorResponse")
    assert len(operation_ids) == len(set(operation_ids))
    assert schema["components"]["securitySchemes"]["BearerAuth"]["scheme"] == "bearer"


def test_production_docs_host_and_origin(config):
    config.environment = "production"
    config.api.auth_required = True
    config.backend = "ble"
    config.api_token = "private-test-secret-" * 3
    config.public_url = "https://mypower.efez.net"
    # Composition is replaced by a no-op lifespan-free service: this test never starts BLE.
    app = create_app(config)
    from httpx import ASGITransport, AsyncClient

    async def check():
        async with AsyncClient(transport=ASGITransport(app), base_url="http://localhost") as client:
            headers = {"Authorization": "Bearer " + config.api_token, "Host": "mypower.efez.net"}
            assert (await client.get("/docs", headers=headers)).status_code == 404
            assert (await client.get("/api/v1/status", headers=headers)).status_code == 200

    asyncio.run(check())


def test_websocket_auth_deadline_and_disconnected_heartbeat(config):
    secret = "safe-test-token-" * 3
    config.api.auth_required = True
    config.api_token = secret
    headers = {"Authorization": "Bearer " + secret}
    with TestClient(create_app(config)) as client:
        with pytest.raises(WebSocketDisconnect):
            with client.websocket_connect("/api/v1/events") as ws:
                before = time.monotonic()
                ws.receive_json()
        assert time.monotonic() - before >= 4.9
        client.put("/api/v1/connection", json={"desired": "paused"}, headers=headers)
        time.sleep(0.1)
        with client.websocket_connect("/api/v1/events", headers=headers) as ws:
            assert ws.receive_json()["type"] == "snapshot"
            assert ws.receive_json()["type"] == "heartbeat"


@pytest.mark.parametrize("seconds", [10, 60, 3600])
def test_history_aggregates_contract_and_request_validation(config, seconds):
    from datetime import UTC, datetime, timedelta

    from mypowers.contracts import HistoryAggregates

    with TestClient(create_app(config)) as client:
        ready(client)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if client.get("/api/v1/status").json()["history"]["state"] == "ok":
                break
            time.sleep(0.02)
        now = datetime.now(UTC)
        params = {
            "since": (now - timedelta(seconds=seconds)).isoformat(),
            "until": (now + timedelta(seconds=1)).isoformat(),
            "bucket_seconds": seconds,
            "limit": 3,
        }
        response = client.get("/api/v1/history/aggregates", params=params)
        assert response.status_code == 200
        result = HistoryAggregates.model_validate(response.json())
        assert result.bucket_seconds == seconds
        assert result.since_ms == int((now - timedelta(seconds=seconds)).timestamp() * 1000)
        assert result.until_ms == int((now + timedelta(seconds=1)).timestamp() * 1000)
        assert len(result.items) <= 3
        assert all(item.sample_count > 0 for item in result.items)
        assert response.headers["cache-control"] == "no-store"
        for change in [
            {"bucket_seconds": 11},
            {"limit": 0},
            {"limit": 257},
            {"since": "2026-10-05T12:00:00"},
            {"since": params["until"]},
            {"since": "1970-01-01T00:00:00Z"},
        ]:
            invalid = client.get("/api/v1/history/aggregates", params=params | change)
            assert invalid.status_code == 422
            assert "error" in invalid.json()
        assert client.get("/api/v1/history/aggregates").status_code == 422
