"""Authenticated HTTP and observational WebSocket adapters."""

import asyncio
import hmac
import json
from contextlib import asynccontextmanager
from typing import Annotated, Any, Literal
from urllib.parse import urlsplit
from uuid import UUID, uuid4

from fastapi import FastAPI, Header, Query, Request, WebSocket
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from starlette.exceptions import HTTPException
from starlette.types import ASGIApp, Message, Receive, Scope, Send
from starlette.websockets import WebSocketDisconnect

from mypowers.config.server import ServerConfig
from mypowers.contracts import (
    AppError,
    Command,
    Connection,
    ConnectionRequest,
    Empty,
    ErrorResponse,
    Level,
    LogLevelRequest,
    LogPage,
    Output,
    OutputRequest,
    Page,
    Status,
    StreamKind,
    StreamMessage,
    utc_now,
)
from mypowers.daemon.service import Service


class Boundaries:
    def __init__(self, app: ASGIApp, config: ServerConfig):
        self.app, self.config = app, config
        self.hosts = {"localhost", "127.0.0.1", "::1", "testserver"}
        self.origins = (
            {f"http://localhost:{config.api.port}", f"http://127.0.0.1:{config.api.port}"}
            if config.environment == "development"
            else set()
        )
        if config.public_url:
            hostname = urlsplit(config.public_url).hostname
            if hostname:
                self.hosts.add(hostname)
            self.origins.add(config.public_url)

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] not in {"http", "websocket"}:
            await self.app(scope, receive, send)
            return
        headers = {
            key.decode("latin-1").lower(): value.decode("latin-1")
            for key, value in scope.get("headers", [])
        }
        try:
            host = urlsplit("//" + headers.get("host", "")).hostname
        except ValueError:
            host = None
        origin = headers.get("origin")
        mutation = scope.get("method") in {"PUT", "POST", "DELETE", "PATCH"}
        code: tuple[str, str, int] | None = None
        if host not in self.hosts or (
            host == "testserver" and self.config.environment == "production"
        ):
            code = ("host_forbidden", "Untrusted Host.", 403)
        elif origin and origin not in self.origins and (mutation or scope["type"] == "websocket"):
            code = ("origin_forbidden", "Untrusted Origin.", 403)
        elif (
            scope["type"] == "http"
            and scope["path"] != "/health/live"
            and self.config.api.auth_required
        ):
            supplied = headers.get("authorization", "")
            if not hmac.compare_digest(
                supplied.encode(), ("Bearer " + (self.config.api_token or "")).encode()
            ):
                code = ("unauthorized", "Valid bearer authentication is required.", 401)
        if code:
            if scope["type"] == "websocket":
                await send({"type": "websocket.close", "code": 1008})
            else:
                await JSONResponse(AppError(*code).payload(str(uuid4())), status_code=code[2])(
                    scope, receive, send
                )
            return
        bounded_receive: Receive
        if scope["type"] == "http" and mutation:
            if (
                scope.get("method") != "DELETE"
                and headers.get("content-type", "").split(";")[0] != "application/json"
            ):
                await JSONResponse(
                    AppError("content_type", "Use application/json.", 422).payload(),
                    status_code=422,
                )(scope, receive, send)
                return
            # Buffer only a bounded body, including chunked requests, before parsing.
            body = bytearray()
            while True:
                chunk = await receive()
                if chunk["type"] == "http.disconnect":
                    return
                body.extend(chunk.get("body", b""))
                if len(body) > 16384:
                    await JSONResponse(
                        AppError("body_too_large", "Request body exceeds 16 KiB.", 413).payload(),
                        status_code=413,
                    )(scope, receive, send)
                    return
                if not chunk.get("more_body", False):
                    break
            delivered = False

            async def receive_buffer() -> Message:
                nonlocal delivered
                if not delivered:
                    delivered = True
                    return {"type": "http.request", "body": bytes(body), "more_body": False}
                return await receive()

            bounded_receive = receive_buffer
        else:
            bounded_receive = receive

        async def no_store(message: Message) -> None:
            if message["type"] == "http.response.start":
                message["headers"] = [*message.get("headers", []), (b"cache-control", b"no-store")]
            await send(message)

        await self.app(scope, bounded_receive, no_store)


def create_app(config: ServerConfig, service: Service | None = None) -> FastAPI:
    runtime = service or Service(config)

    @asynccontextmanager
    async def lifespan(app: FastAPI):  # type: ignore[no-untyped-def]
        await runtime.start()
        try:
            yield
        finally:
            await runtime.close()

    app = FastAPI(
        title="MyPowers",
        version="0.1.0",
        lifespan=lifespan,
        responses={
            code: {"model": ErrorResponse} for code in (401, 403, 404, 409, 413, 422, 429, 500, 503)
        },
        docs_url=None if config.environment == "production" else "/docs",
        redoc_url=None,
        openapi_url=None if config.environment == "production" else "/openapi.json",
    )
    app.state.service = runtime
    app.add_middleware(Boundaries, config=config)

    @app.exception_handler(AppError)
    async def application_error(request: Request, error: AppError) -> JSONResponse:
        return JSONResponse(error.payload(str(uuid4())), status_code=error.status)

    @app.exception_handler(RequestValidationError)
    async def validation_error(request: Request, error: RequestValidationError) -> JSONResponse:
        return JSONResponse(
            AppError("invalid_request", "Request fields or query values are invalid.", 422).payload(
                str(uuid4())
            ),
            status_code=422,
        )

    @app.exception_handler(HTTPException)
    async def http_error(request: Request, error: HTTPException) -> JSONResponse:
        return JSONResponse(
            AppError("http_error", "Requested resource is unavailable.", error.status_code).payload(
                str(uuid4())
            ),
            status_code=error.status_code,
        )

    @app.exception_handler(Exception)
    async def internal_error(request: Request, error: Exception) -> JSONResponse:
        runtime.logs.log(
            "ERROR", "request_failure", "Unexpected API error.", error_type=type(error).__name__
        )
        return JSONResponse(
            AppError("internal_error", "Internal application error.", 500).payload(str(uuid4())),
            status_code=500,
        )

    @app.get("/health/live", operation_id="health_live")
    async def health() -> dict[str, str]:
        return {"status": "ok"}

    @app.get("/api/v1/status", response_model=Status, operation_id="status")
    async def status() -> Status:
        snapshot = runtime.core.snapshot()
        return snapshot.model_copy(
            update={"diagnostics": {**snapshot.diagnostics, **runtime.diagnostics()}}
        )

    @app.get("/api/v1/capabilities", operation_id="capabilities")
    async def capabilities() -> dict[str, Any]:
        return {
            "schema_version": 1,
            "outputs": ["ac", "dc", "light"],
            "metrics": ["battery_percent", "input_power_w", "output_power_w", "remaining_minutes"],
            "profile": config.device.profile,
            "qualified_address": "2A:02:01:48:6B:D0",
            "write_freshness_seconds": 3,
            "confirmation_samples": 2,
        }

    @app.get("/api/v1/history", response_model=Page, operation_id="history")
    async def history(
        since: str | None = None,
        until: str | None = None,
        limit: Annotated[int, Query(ge=1, le=10000)] = 1000,
        cursor: Annotated[str | None, Query(max_length=2048)] = None,
    ) -> Page:
        return await runtime.history.query(since, until, limit, cursor)

    @app.put(
        "/api/v1/outputs/{output}",
        response_model=Command,
        status_code=202,
        operation_id="set_output",
    )
    async def output(
        output: Output, body: OutputRequest, idempotency_key: Annotated[UUID, Header()]
    ) -> Command:
        return runtime.core.admit(output, body, str(idempotency_key))

    @app.get("/api/v1/commands/{command_id}", response_model=Command, operation_id="command")
    async def command(command_id: UUID) -> Command:
        return runtime.core.get_command(str(command_id))

    @app.put("/api/v1/connection", response_model=Connection, operation_id="connection_intent")
    async def connection(body: ConnectionRequest) -> Connection:
        return runtime.core.connection_intent(body.desired)

    @app.post(
        "/api/v1/connection/retry", response_model=Connection, operation_id="connection_retry"
    )
    async def retry(body: Empty) -> Connection:
        runtime.supervisor.wakeup.set()
        return runtime.core.connection

    @app.get("/api/v1/logs", response_model=LogPage, operation_id="logs")
    async def logs(
        tail: Annotated[int | None, Query(ge=1, le=1000)] = None,
        since: str | None = None,
        until: str | None = None,
        min_level: Level = Level.DEBUG,
        limit: Annotated[int, Query(ge=1, le=1000)] = 100,
        cursor: Annotated[str | None, Query(max_length=2048)] = None,
        direction: Literal["forward", "backward"] = "forward",
    ) -> LogPage:
        return await runtime.logs.query(
            tail=tail,
            since=since,
            until=until,
            min_level=min_level,
            limit=limit,
            cursor=cursor,
            direction=direction,
        )

    @app.put("/api/v1/runtime/log-level", operation_id="set_log_level")
    async def set_log_level(body: LogLevelRequest) -> dict[str, Any]:
        return runtime.logs.set_level(body.level, body.duration_seconds)

    @app.delete("/api/v1/runtime/log-level", operation_id="reset_log_level")
    async def reset_log_level() -> dict[str, Any]:
        return runtime.logs.set_level(None)

    ws_count = 0

    async def authenticate(socket: WebSocket) -> bool:
        if any(
            key.lower() in {"token", "api_token", "access_token"} for key in socket.query_params
        ):
            await socket.close(code=1008)
            return False
        await socket.accept()
        if not config.api.auth_required:
            return True
        supplied = socket.headers.get("authorization", "")
        if supplied:
            valid = hmac.compare_digest(
                supplied.encode(), ("Bearer " + (config.api_token or "")).encode()
            )
        else:
            try:
                raw = await asyncio.wait_for(socket.receive_text(), 5)
                data = json.loads(raw) if len(raw.encode()) <= 16384 else {}
                valid = (
                    isinstance(data, dict)
                    and set(data) == {"type", "token"}
                    and data["type"] == "authenticate"
                    and isinstance(data["token"], str)
                    and hmac.compare_digest(
                        data["token"].encode(), (config.api_token or "").encode()
                    )
                )
            except (TimeoutError, ValueError, WebSocketDisconnect):
                valid = False
        if not valid:
            await socket.close(code=1008)
        return valid

    async def stream(socket: WebSocket, log_stream: bool) -> None:
        nonlocal ws_count
        if ws_count >= 16:
            await socket.close(code=1013)
            return
        ws_count += 1
        subscriber: asyncio.Queue[dict[str, Any]] | None = None
        sequence = 0

        async def emit(kind: StreamKind, data: dict[str, Any] | None = None) -> None:
            nonlocal sequence
            sequence += 1
            message = StreamMessage(
                type=kind,
                server_instance_id=runtime.core.instance,
                stream_sequence=sequence,
                server_time=utc_now(),
                data=data,
            )
            await asyncio.wait_for(socket.send_json(message.model_dump(mode="json")), 2)

        async def incoming() -> None:
            while True:
                message = await socket.receive()
                if message["type"] == "websocket.disconnect":
                    return
                # Streams are observational. Unexpected client messages are never commands.
                raise AppError(
                    "unexpected_stream_message", "No stream mutations are supported.", 422
                )

        reader: asyncio.Task[None] | None = None
        next_event: asyncio.Task[dict[str, Any]] | None = None
        try:
            if not await authenticate(socket):
                return
            subscriber = runtime.logs.subscribe() if log_stream else runtime.core.subscribe()
            await emit("snapshot", runtime.core.snapshot().model_dump(mode="json"))
            retained_ids: set[tuple[str, int]] = set()
            if log_stream:
                level = Level(socket.query_params.get("min_level", "DEBUG"))
                cursor = socket.query_params.get("cursor")
                page = await runtime.logs.query(
                    tail=None if cursor else 10, cursor=cursor, min_level=level
                )
                if page.gap:
                    await emit("gap", {"source": page.source})
                for record in page.items:
                    retained_ids.add((record["server_instance_id"], record["sequence"]))
                    await emit("log", record)
            else:
                level = Level.DEBUG
            reader = asyncio.create_task(incoming())
            while not runtime.closing:
                next_event = asyncio.create_task(subscriber.get())
                done, _ = await asyncio.wait(
                    {reader, next_event}, timeout=5, return_when=asyncio.FIRST_COMPLETED
                )
                if reader in done:
                    next_event.cancel()
                    await asyncio.gather(next_event, return_exceptions=True)
                    await reader
                    break
                if next_event not in done:
                    next_event.cancel()
                    await asyncio.gather(next_event, return_exceptions=True)
                    await emit("heartbeat")
                    continue
                event = next_event.result()
                if (
                    event.get("type") in {"overflow", "shutdown"}
                    or event.get("event") == "stream_overflow"
                ):
                    await socket.close(code=1013)
                    break
                if log_stream:
                    identity = (event["server_instance_id"], event["sequence"])
                    if identity in retained_ids:
                        retained_ids.discard(identity)
                        continue
                    if runtime.logs.matches(
                        event, {"start": None, "end": None, "level": level.value}
                    ):
                        await emit("log", event)
                else:
                    await emit(
                        event["type"],
                        event.get("data") or runtime.core.snapshot().model_dump(mode="json"),
                    )
        except AppError as error:
            if socket.application_state.name == "CONNECTED":
                await socket.close(
                    code=1013 if error.status in {429, 503} else 1008, reason=error.code
                )
        except asyncio.CancelledError:
            pass
        except TimeoutError:
            try:
                await asyncio.wait_for(socket.close(code=1013, reason="stream_send_timeout"), 0.5)
            except (TimeoutError, RuntimeError):
                pass
        except (WebSocketDisconnect, RuntimeError, ValueError):
            pass
        finally:
            if next_event and not next_event.done():
                next_event.cancel()
                await asyncio.gather(next_event, return_exceptions=True)
            if reader:
                reader.cancel()
                await asyncio.gather(reader, return_exceptions=True)
            if subscriber is not None:
                (runtime.logs.subscribers if log_stream else runtime.core.subscribers).discard(
                    subscriber
                )
                runtime.core.slow_subscribers.discard(subscriber)
            ws_count -= 1

    @app.websocket("/api/v1/events")
    async def events(socket: WebSocket) -> None:
        await stream(socket, False)

    @app.websocket("/api/v1/logs/stream")
    async def log_events(socket: WebSocket) -> None:
        await stream(socket, True)

    schema = app.openapi()
    schema.setdefault("components", {})["securitySchemes"] = {
        "BearerAuth": {"type": "http", "scheme": "bearer"}
    }
    schema["security"] = [{"BearerAuth": []}]
    schema["paths"]["/health/live"]["get"]["security"] = []
    return app
