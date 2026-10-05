"""Transport-only contracts; clients never import server infrastructure."""

import re
from datetime import UTC, datetime
from enum import StrEnum
from typing import Any, Literal
from uuid import UUID
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import BaseModel, ConfigDict, Field, StrictBool, field_validator


class DTO(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class Output(StrEnum):
    AC = "ac"
    DC = "dc"
    LIGHT = "light"


class Level(StrEnum):
    DEBUG = "DEBUG"
    INFO = "INFO"
    WARNING = "WARNING"
    ERROR = "ERROR"


class OutputRequest(DTO):
    enabled: StrictBool
    server_instance_id: UUID
    expected_outputs_revision: int = Field(strict=True, ge=0)


class ConnectionRequest(DTO):
    desired: Literal["running", "paused"]


class Empty(DTO):
    pass


class LogLevelRequest(DTO):
    level: Level
    duration_seconds: float | None = Field(default=None, gt=0, le=86400, allow_inf_nan=False)


class Preferences(DTO):
    graph_interval_seconds: Literal[10, 30, 60, 3600] = 10
    graph_visualization: Literal["sparkline", "chart"] = "sparkline"
    graph_base_scale_w: Literal[100, 300] = 100
    timezone: str = Field(default="system", max_length=128)
    logs_page_size: Literal[50, 100, 250, 500, 1000] = 100

    @field_validator(
        "graph_interval_seconds", "graph_base_scale_w", "logs_page_size", mode="before"
    )
    @classmethod
    def integer_value(cls, value: Any) -> int:
        if type(value) is not int:
            raise ValueError("Use an integer value.")
        return value

    @field_validator("timezone")
    @classmethod
    def valid_timezone(cls, value: str) -> str:
        if value != "system":
            try:
                ZoneInfo(value)
            except (ValueError, ZoneInfoNotFoundError):
                raise ValueError("Use system or an IANA timezone.") from None
        return value


class Settings(Preferences):
    schema_version: Literal[1] = 1


class SettingsUpdate(Preferences):
    """Only explicitly supplied fields are written to SQLite."""


class Sample(DTO):
    sequence: int
    received_at: str
    segment_id: str
    battery_percent: int = Field(ge=0, le=100)
    input_power_w: int = Field(ge=0, le=65535)
    output_power_w: int = Field(ge=0, le=65535)
    remaining_minutes: int = Field(ge=0, le=65535)
    ac_enabled: bool
    dc_enabled: bool
    light_enabled: bool
    status_flags: int = Field(ge=0, le=127)


CommandStatus = Literal[
    "accepted",
    "waiting_for_status",
    "sent",
    "confirmed",
    "no_change",
    "rejected",
    "failed",
    "unconfirmed",
]
TERMINAL = frozenset({"confirmed", "no_change", "rejected", "failed", "unconfirmed"})


class Command(DTO):
    schema_version: Literal[1] = 1
    command_id: str
    status: CommandStatus = "accepted"
    output: Output
    requested_enabled: bool
    created_at: str
    completed_at: str | None = None
    confirmed_at: str | None = None
    reason_code: str | None = None
    observed_flags: int | None = None
    source_flags: int | None = None
    target_flags: int | None = None
    session_id: str | None = None
    confirmation_sequences: tuple[int, ...] = ()


class Connection(DTO):
    phase: str = "starting"
    reason_code: str | None = None
    message: str = "Starting."
    hints: tuple[str, ...] = ()
    desired: Literal["running", "paused"] = "running"
    link_connected: bool = False
    session_id: str | None = None
    adapter_address: str
    adapter_name: str = "Actions"
    adapter_id: str | None = None
    retry_in_seconds: float | None = None


TelemetryState = Literal["unknown", "waiting", "live", "stale", "invalid"]
StreamKind = Literal["snapshot", "state", "command", "heartbeat", "log", "gap"]


class Telemetry(DTO):
    state: TelemetryState
    age_seconds: float | None
    sample: Sample | None


class Controls(DTO):
    supported_outputs: tuple[Output, ...] = tuple(Output)
    allowed: bool
    reason_code: str | None
    outputs_revision: int
    pending_command_id: str | None


class Status(DTO):
    schema_version: Literal[1] = 1
    server_time: str
    server_instance_id: str
    state_version: int
    server: dict[str, Any]
    device: dict[str, Any]
    connection: Connection
    telemetry: Telemetry
    controls: Controls
    history: dict[str, Any]
    logging: dict[str, Any]
    diagnostics: dict[str, Any]


class Page(DTO):
    schema_version: Literal[1] = 1
    items: list[dict[str, Any]]
    next_cursor: str | None = None
    source: str = "database"
    gap: bool = False
    skipped_lines: int = 0


class LogPage(Page):
    previous_cursor: str | None = None
    has_more_before: bool = False
    has_more_after: bool = False


class HistoryBucket(DTO):
    bucket_start_ms: int
    input_power_w: float = Field(ge=0, le=65535, allow_inf_nan=False)
    output_power_w: float = Field(ge=0, le=65535, allow_inf_nan=False)
    sample_count: int = Field(ge=1)


class HistoryAggregates(DTO):
    schema_version: Literal[1] = 1
    bucket_seconds: Literal[10, 30, 60, 3600]
    since_ms: int
    until_ms: int
    items: list[HistoryBucket]
    source: Literal["database"] = "database"


class StreamMessage(DTO):
    schema_version: Literal[1] = 1
    type: StreamKind
    server_instance_id: str
    stream_sequence: int
    server_time: str
    data: dict[str, Any] | None = None


class ErrorDetail(DTO):
    code: str
    message: str
    retryable: bool
    request_id: str


class ErrorResponse(DTO):
    schema_version: Literal[1] = 1
    error: ErrorDetail


class AppError(Exception):
    def __init__(self, code: str, message: str, status: int = 409, retryable: bool = False):
        super().__init__(message)
        self.code, self.message, self.status, self.retryable = code, message, status, retryable

    def payload(self, request_id: str = "") -> dict[str, Any]:
        return {
            "schema_version": 1,
            "error": {
                "code": self.code,
                "message": self.message,
                "retryable": self.retryable,
                "request_id": request_id,
            },
        }


def utc_now() -> str:
    return timestamp(datetime.now(UTC).timestamp())


def timestamp(epoch: float) -> str:
    return (
        datetime.fromtimestamp(epoch, UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")
    )


def aware_ms(value: str) -> int:
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            raise ValueError
        return int(parsed.timestamp() * 1000)
    except (ValueError, OverflowError):
        raise AppError("invalid_time", "Use an aware RFC3339 timestamp.", 422) from None


def safe_text(value: str) -> str:
    return re.sub(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]", "", value)
