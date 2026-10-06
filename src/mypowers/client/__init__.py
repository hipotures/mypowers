"""Python API helper for capture scripts and tests; native clients use Rust."""

import asyncio
import json
import ssl
import time
from collections.abc import AsyncIterator
from typing import Any
from uuid import UUID, uuid4

import httpx
from pydantic import ValidationError
from websockets.asyncio.client import connect

from mypowers.config import ClientConfig
from mypowers.contracts import TERMINAL, AppError, Command, Output, Status, StreamMessage


class Client:
    def __init__(self, config: ClientConfig):
        self.config = config
        self.headers = {"Authorization": "Bearer " + config.api_token} if config.api_token else {}
        self.tls = (
            ssl.create_default_context(cafile=config.ca_file)
            if config.ca_file
            else ssl.create_default_context()
        )
        self.http = httpx.AsyncClient(
            base_url=config.server,
            headers=self.headers,
            timeout=config.timeout,
            verify=self.tls,
            trust_env=False,
            follow_redirects=False,
        )

    async def __aenter__(self) -> "Client":
        return self

    async def __aexit__(self, *args: object) -> None:
        await self.http.aclose()

    async def request(
        self,
        method: str,
        path: str,
        *,
        body: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
        key: str | None = None,
    ) -> dict[str, Any]:
        try:
            response = await self.http.request(
                method,
                "/api/v1" + path,
                json=body,
                params={k: v for k, v in (params or {}).items() if v is not None},
                headers={"Idempotency-Key": key} if key else None,
            )
        except httpx.HTTPError:
            raise AppError(
                "server_unreachable", "Cannot reach server or verify TLS.", 0, True
            ) from None
        if len(response.content) > 16 * 1024 * 1024:
            raise AppError("incompatible_response", "Server response exceeds the client limit.", 0)
        try:
            data = response.json()
            if not isinstance(data, dict):
                raise ValueError
        except ValueError:
            raise AppError("incompatible_response", "Server returned invalid JSON.", 0) from None
        if response.is_error:
            error = data.get("error", {})
            raise AppError(
                str(error.get("code", "http_error")),
                str(error.get("message", "API error.")),
                response.status_code,
                bool(error.get("retryable", False)),
            )
        if not 200 <= response.status_code < 300:
            raise AppError("unexpected_redirect", "Server redirect refused.", 0)
        return data

    async def status(self) -> Status:
        try:
            return Status.model_validate(await self.request("GET", "/status"))
        except ValidationError:
            raise AppError(
                "incompatible_response", "Server status does not match API v1.", 0
            ) from None

    async def admit(self, output: Output, enabled: bool, snapshot: Status | None = None) -> Command:
        current = snapshot or await self.status()
        key = str(uuid4())
        try:
            data = await self.request(
                "PUT",
                f"/outputs/{output.value}",
                key=key,
                body={
                    "enabled": enabled,
                    "server_instance_id": current.server_instance_id,
                    "expected_outputs_revision": current.controls.outputs_revision,
                },
            )
            return Command.model_validate(data)
        except AppError as error:
            if error.status == 0:
                raise AppError(
                    "admission_unknown",
                    f"Admission uncertain; idempotency key {key}. Do not replay.",
                    0,
                ) from None
            raise
        except ValidationError:
            raise AppError(
                "admission_unknown",
                f"Invalid admission response; idempotency key {key}. Do not replay.",
                0,
            ) from None

    async def command(self, command_id: str) -> Command:
        try:
            UUID(command_id)
            return Command.model_validate(await self.request("GET", f"/commands/{command_id}"))
        except (ValidationError, ValueError):
            raise AppError(
                "incompatible_response", "Invalid command identifier or response.", 0
            ) from None

    async def wait_command(self, command: Command, budget: float = 25) -> Command:
        deadline = time.monotonic() + budget
        while command.status not in TERMINAL:
            if time.monotonic() >= deadline:
                return command.model_copy(
                    update={"status": "unconfirmed", "reason_code": "client_wait_expired"}
                )
            await asyncio.sleep(0.2)
            try:
                command = await self.command(command.command_id)
            except AppError:
                return command.model_copy(
                    update={"status": "unconfirmed", "reason_code": "client_connection_lost"}
                )
        return command

    async def stream(self, logs: bool = False, **params: str) -> AsyncIterator[StreamMessage]:
        from urllib.parse import urlencode

        origin = self.config.server.replace("https://", "wss://").replace("http://", "ws://")
        path = "/api/v1/logs/stream" if logs else "/api/v1/events"
        query = "?" + urlencode(params) if params else ""
        async with connect(
            origin + path + query,
            additional_headers=self.headers,
            ssl=self.tls if origin.startswith("wss:") else None,
            proxy=None,
            max_size=32 * 1024,
            max_queue=128,
            open_timeout=self.config.timeout,
        ) as ws:
            last_sequence = 0
            instance: str | None = None
            while True:
                raw = await asyncio.wait_for(ws.recv(), 15)
                try:
                    message = StreamMessage.model_validate(json.loads(raw))
                except (ValidationError, ValueError):
                    raise AppError(
                        "incompatible_stream", "Invalid API stream message.", 0
                    ) from None
                if message.stream_sequence <= last_sequence:
                    raise AppError("incompatible_stream", "Nonmonotonic stream sequence.", 0)
                if instance is None:
                    if message.type != "snapshot":
                        raise AppError("incompatible_stream", "Missing initial snapshot.", 0)
                    instance = message.server_instance_id
                elif message.server_instance_id != instance:
                    raise AppError(
                        "incompatible_stream", "Server instance changed within stream.", 0
                    )
                last_sequence = message.stream_sequence
                yield message
