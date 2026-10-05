"""Immutable observations and serialized daemon-owned command operations."""

import asyncio
import time
from collections import OrderedDict
from collections.abc import Callable
from dataclasses import dataclass, replace
from typing import Any, Protocol
from uuid import uuid4

from mypowers import __version__
from mypowers.contracts import (
    AppError,
    Command,
    CommandStatus,
    Connection,
    Controls,
    Output,
    OutputRequest,
    Sample,
    Status,
    Telemetry,
    TelemetryState,
    timestamp,
    utc_now,
)
from mypowers.protocol import (
    PROFILE,
    PROFILES,
    STATION_ADDRESS,
    STATION_NAME,
    Reading,
    decode,
    encode,
    target_flags,
)


class Clock(Protocol):
    def monotonic(self) -> float: ...
    def time(self) -> float: ...


class SystemClock:
    monotonic = staticmethod(time.monotonic)
    time = staticmethod(time.time)


class Writer(Protocol):
    async def write(self, frame: bytes) -> None: ...


@dataclass(frozen=True, slots=True)
class Observation:
    sample: Sample
    monotonic: float
    epoch: float
    session: str


@dataclass(frozen=True, slots=True)
class Notification:
    observation: Observation | None
    sequence: int
    session: str
    monotonic: float


@dataclass(frozen=True, slots=True)
class Retained:
    key: str
    request: OutputRequest
    output: Output
    command: Command
    admitted: float


class Core:
    def __init__(
        self,
        *,
        address: str,
        name: str,
        profile: str,
        adapter: str,
        environment: str = "development",
        backend: str = "simulated",
        stale_after: float = 3,
        clock: Clock | None = None,
    ):
        self.clock = clock or SystemClock()
        self.instance = str(uuid4())
        self.started = self.clock.monotonic()
        self.address, self.name, self.profile = address, name, profile
        self.environment, self.backend = environment, backend
        self.stale_after = min(stale_after, 3)
        self.connection = Connection(adapter_address=adapter)
        self.latest: Observation | None = None
        self.segment = str(uuid4())
        self.sequence = self.revision = self.state_version = self.valid_samples = self.rejected = 0
        self.invalid = False
        self.synchronized = 0
        self.resync_required = False
        self.writer: Writer | None = None
        self.pending: str | None = None
        self.commands: OrderedDict[str, Retained] = OrderedDict()
        self.command_task: asyncio.Task[None] | None = None
        self.notifications: asyncio.Queue[Notification] = asyncio.Queue(128)
        self.subscribers: set[asyncio.Queue[dict[str, Any]]] = set()
        self.slow_subscribers: set[asyncio.Queue[dict[str, Any]]] = set()
        self.closing = False
        self.history_health: Callable[[], dict[str, Any]] = lambda: {
            "enabled": False,
            "state": "disabled",
        }
        self.log_health: Callable[[], dict[str, Any]] = lambda: {"state": "starting"}
        self.log: Callable[..., None] = lambda *args, **kwargs: None
        self.wake: Callable[[], None] = lambda: None
        self.last_eligible = False
        self.reconnect_attempts = 0
        self.rssi: int | None = None
        self.rssi_at: float | None = None
        self.rssi_monotonic: float | None = None

    def subscribe(self, capacity: int = 128) -> asyncio.Queue[dict[str, Any]]:
        if len(self.subscribers) >= 16:
            raise AppError("stream_limit", "Too many event streams.", 429, True)
        queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue(capacity)
        self.subscribers.add(queue)
        return queue

    def publish(self, kind: str = "state", data: dict[str, Any] | None = None) -> None:
        self.state_version += 1
        event = {"type": kind, "data": data}
        for queue in self.subscribers:
            if queue in self.slow_subscribers:
                continue
            if queue.full():
                # A sentinel explicitly closes a slow stream; command results remain queryable.
                while not queue.empty():
                    queue.get_nowait()
                self.slow_subscribers.add(queue)
                queue.put_nowait({"type": "overflow"})
            else:
                queue.put_nowait(event)

    def invalidate(self) -> None:
        self.revision += 1
        self.last_eligible = False
        self.segment = str(uuid4())

    def set_phase(
        self, phase: str, reason: str | None = None, message: str | None = None, **updates: Any
    ) -> None:
        previous = self.connection
        self.connection = previous.model_copy(
            update={
                "phase": phase,
                "reason_code": reason,
                "message": message or phase.replace("_", " ").capitalize() + ".",
                "retry_in_seconds": None,
                **updates,
            }
        )
        if not self.connection.link_connected and previous.link_connected:
            self.invalidate()
            self.synchronized = 0
            self._notify(None)
        if previous != self.connection:
            self.log(
                "INFO",
                "connection_transition",
                self.connection.message,
                phase=phase,
                reason_code=reason,
            )
            self.publish()

    def begin_session(self, writer: Writer, adapter_id: str) -> str:
        self.invalidate()
        self.writer = writer
        self.synchronized = 0
        self.invalid = False
        session = str(uuid4())
        self.set_phase(
            "waiting_for_telemetry", link_connected=True, session_id=session, adapter_id=adapter_id
        )
        return session

    def end_session(self) -> None:
        self.writer = None
        self.synchronized = 0
        self.invalidate()
        self.set_phase("reconnecting", link_connected=False, session_id=None)

    def receive(self, frame: bytes, session: str) -> None:
        if session != self.connection.session_id or not self.connection.link_connected:
            return
        self.sequence += 1
        self.log("DEBUG", "ble_rx", "Received station frame.", frame_hex=frame.hex())
        try:
            reading = decode(frame)
        except ValueError:
            self.rejected += 1
            if not self.invalid:
                self.invalidate()
            self.invalid = True
            self.synchronized = 0
            self._notify(None)
            self.publish()
            return
        mono, epoch = self.clock.monotonic(), self.clock.time()
        previous = self.latest
        if previous and mono - previous.monotonic >= self.stale_after:
            # A fresh frame after silence starts a new history segment even when
            # no API read or periodic tick observed the stale interval.
            self.segment = str(uuid4())
        if previous and abs((epoch - previous.epoch) - (mono - previous.monotonic)) > 2:
            self.segment = str(uuid4())
            self.log("WARNING", "clock_discontinuity", "Wall clock discontinuity observed.")
        if previous and previous.sample.status_flags != reading.flags:
            self.revision += 1
        self.invalid = False
        self.synchronized = self.synchronized + 1 if reading.flags in PROFILES else 0
        sample = self.sample(reading, epoch)
        self.latest = Observation(sample, mono, epoch, session)
        self.valid_samples += 1
        self.set_phase("connected", link_connected=True)
        self._notify(self.latest)
        self.last_eligible = self.eligible_reason() is None
        self.publish()

    def sample(self, reading: Reading, epoch: float) -> Sample:
        return Sample(
            sequence=self.sequence,
            received_at=timestamp(epoch),
            segment_id=self.segment,
            battery_percent=reading.battery,
            input_power_w=reading.input_w,
            output_power_w=reading.output_w,
            remaining_minutes=reading.minutes,
            ac_enabled=bool(reading.flags & 2),
            dc_enabled=bool(reading.flags & 1),
            light_enabled=bool(reading.flags & 16),
            status_flags=reading.flags,
        )

    def _notify(self, observation: Observation | None) -> None:
        if self.pending:
            event = Notification(
                observation, self.sequence, self.connection.session_id or "", self.clock.monotonic()
            )
            if self.notifications.full():
                while not self.notifications.empty():
                    self.notifications.get_nowait()
                event = replace(event, observation=None)
            self.notifications.put_nowait(event)

    def age(self) -> float | None:
        if not self.latest:
            return None
        return max(0, self.clock.monotonic() - self.latest.monotonic)

    def telemetry_state(self) -> TelemetryState:
        if self.invalid:
            return "invalid"
        if self.latest is None:
            return "waiting" if self.connection.link_connected else "unknown"
        age = self.age()
        if (
            not self.connection.link_connected
            or self.latest.session != self.connection.session_id
            or age is None
            or age >= self.stale_after
        ):
            return "stale"
        return "live"

    def eligible_reason(self) -> str | None:
        if self.closing:
            return "shutting_down"
        if self.connection.desired == "paused":
            return "paused"
        if self.address != STATION_ADDRESS or self.name != STATION_NAME or self.profile != PROFILE:
            return "unqualified_identity"
        if self.telemetry_state() != "live":
            return "telemetry_unavailable"
        if self.latest is None or self.latest.sample.status_flags not in PROFILES:
            return "unqualified_profile"
        if self.resync_required or self.synchronized < 2:
            return "resynchronizing"
        if not self.writer:
            return "telemetry_unavailable"
        return None

    def tick(self) -> None:
        if self.last_eligible and self.eligible_reason() is not None:
            self.invalidate()
            self._notify(None)
            self.publish()

    def snapshot(self) -> Status:
        self.tick()
        reason = self.eligible_reason()
        return Status(
            server_time=timestamp(self.clock.time()),
            server_instance_id=self.instance,
            state_version=self.state_version,
            server={
                "version": __version__,
                "environment": self.environment,
                "backend": self.backend,
                "uptime_seconds": self.clock.monotonic() - self.started,
            },
            device={
                "id": "s300",
                "address": self.address,
                "name": self.name,
                "profile": self.profile,
                "hardware_version": None,
                "firmware_version": None,
                "nominal_user_metadata": {"capacity_wh": 288, "continuous_w": 300, "peak_w": 500},
            },
            connection=self.connection,
            telemetry=Telemetry(
                state=self.telemetry_state(),
                age_seconds=self.age(),
                sample=self.latest.sample if self.latest else None,
            ),
            controls=Controls(
                allowed=reason is None and not self.pending,
                reason_code="command_busy" if self.pending else reason,
                outputs_revision=self.revision,
                pending_command_id=self.pending,
            ),
            history=self.history_health(),
            logging=self.log_health(),
            diagnostics={
                "valid_samples": self.valid_samples,
                "rejected_frames": self.rejected,
                "reconnect_attempts": self.reconnect_attempts,
                "last_scan_rssi_dbm": self.rssi,
                "last_scan_rssi_received_at": timestamp(self.rssi_at) if self.rssi_at else None,
                "last_scan_rssi_age_seconds": max(0, self.clock.monotonic() - self.rssi_monotonic)
                if self.rssi_monotonic is not None
                else None,
            },
        )

    def prune(self) -> None:
        now = self.clock.monotonic()
        for command_id, record in list(self.commands.items()):
            if command_id != self.pending and (
                now - record.admitted >= 3600 or len(self.commands) > 1000
            ):
                del self.commands[command_id]

    def admit(self, output: Output, request: OutputRequest, key: str) -> Command:
        self.prune()
        for record in self.commands.values():
            if record.key == key:
                if record.request != request or record.output != output:
                    raise AppError(
                        "idempotency_conflict", "Key already used with a different intention."
                    )
                return record.command
        if str(request.server_instance_id) != self.instance:
            raise AppError("state_conflict", "Server restarted; refresh status.")
        if self.pending:
            raise AppError("command_busy", "An output operation is in progress.")
        self.tick()
        if request.expected_outputs_revision != self.revision:
            raise AppError(
                "state_conflict", "Station output state changed; refresh before retrying."
            )
        reason = self.eligible_reason()
        if reason:
            raise AppError(
                reason,
                "Fresh qualified station data is required.",
                409
                if reason == "unqualified_profile"
                else 403
                if reason == "unqualified_identity"
                else 503,
            )
        command = Command(
            command_id=str(uuid4()),
            output=output,
            requested_enabled=request.enabled,
            created_at=timestamp(self.clock.time()),
        )
        if len(self.commands) >= 1000:
            oldest = next(iter(self.commands))
            del self.commands[oldest]
        self.commands[command.command_id] = Retained(
            key, request, output, command, self.clock.monotonic()
        )
        self.pending = command.command_id
        while not self.notifications.empty():
            self.notifications.get_nowait()
        self.command_task = asyncio.create_task(
            self.execute(command.command_id), name="output-command"
        )
        self.publish("command", command.model_dump(mode="json"))
        return command

    def get_command(self, command_id: str) -> Command:
        self.prune()
        if command_id not in self.commands:
            raise AppError("command_not_found", "Command is unknown or expired.", 404)
        return self.commands[command_id].command

    def update_command(self, command_id: str, status: CommandStatus, **updates: Any) -> None:
        record = self.commands[command_id]
        command = record.command.model_copy(update={"status": status, **updates})
        self.commands[command_id] = replace(record, command=command)
        self.publish("command", command.model_dump(mode="json"))

    async def next_observation(self, session: str, cutoff: int, budget: float) -> Observation:
        deadline = self.clock.monotonic() + budget
        while True:
            if self.connection.session_id != session or not self.connection.link_connected:
                raise AppError("connection_lost", "BLE connection lost.", 503)
            remaining = deadline - self.clock.monotonic()
            if remaining <= 0:
                raise AppError(
                    "status_timeout", "No new qualified status before the deadline.", 503
                )
            event = await asyncio.wait_for(self.notifications.get(), remaining)
            if event.session != session or event.sequence <= cutoff:
                continue
            if event.observation is None:
                raise AppError(
                    "invalid_or_lost_telemetry", "Telemetry became invalid or unavailable.", 503
                )
            observation = event.observation
            if self.clock.monotonic() - observation.monotonic >= self.stale_after:
                raise AppError("stale_telemetry", "Telemetry became stale.", 503)
            return observation

    async def execute(self, command_id: str) -> None:
        attempted = False
        record = self.commands[command_id]
        session = self.connection.session_id or ""
        cutoff = self.sequence
        try:
            self.update_command(command_id, "waiting_for_status")
            before = await self.next_observation(session, cutoff, 3)
            if record.request.expected_outputs_revision != self.revision:
                raise AppError("state_conflict", "Output state changed while waiting.")
            reason = self.eligible_reason()
            if reason:
                raise AppError(reason, "Control eligibility was revoked.", 503)
            source = before.sample.status_flags
            target = target_flags(source, record.output, record.request.enabled)
            self.update_command(
                command_id,
                "waiting_for_status",
                source_flags=source,
                target_flags=target,
                session_id=session,
                observed_flags=source,
            )
            if source == target:
                self.update_command(command_id, "no_change")
                return
            writer = self.writer
            if not writer or self.connection.session_id != session:
                raise AppError("connection_lost", "BLE session was replaced.", 503)
            cutoff, transmitted = self.sequence, self.clock.monotonic()
            frame = encode(target)
            self.log(
                "DEBUG",
                "ble_tx",
                "Sending one qualified output frame.",
                frame_hex=frame.hex(),
                command_id=command_id,
            )
            attempted = True
            self.update_command(command_id, "sent")
            await asyncio.wait_for(writer.write(frame), 3)
            confirmations: list[int] = []
            deadline = transmitted + 10
            while len(confirmations) < 2:
                observed = await self.next_observation(
                    session, cutoff, min(3, max(0, deadline - self.clock.monotonic()))
                )
                cutoff = observed.sample.sequence
                if observed.monotonic <= transmitted:
                    continue
                flags = observed.sample.status_flags
                self.update_command(command_id, "sent", observed_flags=flags)
                if flags == target:
                    confirmations.append(cutoff)
                elif flags != source or confirmations:
                    raise AppError(
                        "confirmation_contradiction", "Unexpected complete output state."
                    )
            self.update_command(
                command_id,
                "confirmed",
                confirmed_at=utc_now(),
                confirmation_sequences=tuple(confirmations),
            )
        except asyncio.CancelledError:
            self.update_command(
                command_id, "unconfirmed" if attempted else "failed", reason_code="shutdown"
            )
            if attempted:
                self.resync_required = True
            raise
        except Exception as error:
            code = error.code if isinstance(error, AppError) else "transport_or_status_timeout"
            self.update_command(
                command_id, "unconfirmed" if attempted else "rejected", reason_code=code
            )
            if attempted:
                self.resync_required = True
                self.invalidate()
                self.wake()
        finally:
            self.update_command(
                command_id,
                self.commands[command_id].command.status,
                completed_at=timestamp(self.clock.time()),
                observed_flags=self.latest.sample.status_flags if self.latest else None,
            )
            final = self.commands[command_id].command
            self.log(
                "INFO",
                "command_outcome",
                "Output operation completed.",
                command_id=command_id,
                status=final.status,
                reason_code=final.reason_code,
                source_flags=final.source_flags,
                target_flags=final.target_flags,
                confirmation_sequences=final.confirmation_sequences,
            )
            self.pending = None
            self.publish()

    def connection_intent(self, desired: str) -> Connection:
        if self.closing:
            raise AppError("shutting_down", "Server is shutting down.", 503)
        if self.pending and desired == "paused":
            raise AppError("command_busy", "Cannot pause during an output operation.")
        self.connection = self.connection.model_copy(update={"desired": desired})
        self.wake()
        self.publish()
        return self.connection

    async def close(self) -> None:
        self.closing = True
        if self.command_task and not self.command_task.done():
            self.command_task.cancel()
            await asyncio.gather(self.command_task, return_exceptions=True)

        if self.pending:
            command = self.commands[self.pending].command
            if command.status not in {
                "confirmed",
                "no_change",
                "rejected",
                "failed",
                "unconfirmed",
            }:
                self.update_command(
                    self.pending,
                    "unconfirmed" if command.status == "sent" else "failed",
                    reason_code="shutdown",
                    completed_at=timestamp(self.clock.time()),
                )
            self.pending = None
