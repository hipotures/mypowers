"""SQLite telemetry persistence and validated application settings, without raw frames or logs."""

import asyncio
import base64
import hashlib
import hmac
import json
import logging
import math
import os
import sqlite3
import time
from pathlib import Path
from typing import Any, Literal, cast
from uuid import uuid4

import aiosqlite

from mypowers.contracts import (
    AppError,
    HistoryAggregates,
    HistoryBucket,
    Page,
    Settings,
    SettingsUpdate,
    aware_ms,
)
from mypowers.core import Core, Observation
from mypowers.storage.schema import AGGREGATES, INSERT_STATE, SCHEMA, STATE_FIELDS, VERSION

SETTINGS_SCHEMA = """
CREATE TABLE IF NOT EXISTS settings (
 key TEXT PRIMARY KEY, value_json TEXT NOT NULL
) STRICT;
"""


class CursorCodec:
    def __init__(self) -> None:
        self.secret = os.urandom(32)

    def encode(self, data: dict[str, Any]) -> str:
        raw = json.dumps(data, separators=(",", ":"), sort_keys=True).encode()
        signature = hmac.digest(self.secret, raw, "sha256")
        return base64.urlsafe_b64encode(signature + raw).decode()

    def decode(self, value: str) -> dict[str, Any]:
        try:
            if len(value) > 2048:
                raise ValueError
            raw = base64.b64decode(value, altchars=b"-_", validate=True)
            if not hmac.compare_digest(raw[:32], hmac.digest(self.secret, raw[32:], "sha256")):
                raise ValueError
            data = json.loads(raw[32:])
            if not isinstance(data, dict):
                raise ValueError
            return data
        except (ValueError, TypeError):
            raise AppError(
                "invalid_cursor", "Cursor is invalid or belongs to an earlier instance.", 422
            ) from None


class HistoryStore:
    def __init__(self, core: Core, directory: Path, enabled: bool, interval: float):
        self.core, self.path, self.enabled, self.interval = (
            core,
            directory / "mypowers.db",
            enabled,
            interval,
        )
        self.db: aiosqlite.Connection | None = None
        self.device_id: int | None = None
        self.state = "starting" if enabled else "disabled"
        self.error: str | None = None
        self.last_write: str | None = None
        self.dropped = 0
        self.saved_sequence = 0
        self.last_segment: str | None = None
        self.last_bucket: int | None = None
        self.saved_values: tuple[Any, ...] | None = None
        self.queue: asyncio.Queue[Observation] = asyncio.Queue(256)
        self.lock = asyncio.Lock()
        self.queries = 0
        self.codec = CursorCodec()
        self.instance = str(uuid4())
        self.stopping = False
        self.core.history_health = self.health

    def health(self) -> dict[str, Any]:
        return {
            "enabled": self.enabled,
            "state": self.state,
            "interval_seconds": self.interval,
            "database_path": str(self.path),
            "last_successful_write": self.last_write,
            "pending_count": self.queue.qsize(),
            "dropped_samples": self.dropped,
            "error": self.error,
            "sqlite_version": sqlite3.sqlite_version,
        }

    async def open(self) -> None:
        if sqlite3.sqlite_version_info < (3, 37, 0):
            raise RuntimeError("SQLite 3.37 or newer is required for STRICT tables.")
        await asyncio.to_thread(self.path.parent.mkdir, parents=True, exist_ok=True)
        connection = await aiosqlite.connect(self.path, timeout=5)
        try:
            async with connection.execute("PRAGMA user_version") as cursor:
                row = await cursor.fetchone()
            version = row[0] if row else -1
            if version == 0:
                async with connection.execute(
                    "SELECT name FROM sqlite_master WHERE type='table'"
                ) as cursor:
                    if await cursor.fetchall():
                        raise RuntimeError(
                            "Unversioned nonempty database requires operator action."
                        )
            elif version != VERSION:
                raise RuntimeError(
                    "Unsupported database schema; run scripts/migrate-history.py on a backup."
                )
            for name, value in (
                ("journal_mode", "DELETE"),
                ("synchronous", "FULL"),
                ("foreign_keys", "ON"),
                ("busy_timeout", "5000"),
            ):
                await connection.execute(f"PRAGMA {name}={value}")
            if version == 0:
                await connection.executescript(SCHEMA)
            for name, expected in (
                ("journal_mode", "delete"),
                ("synchronous", 2),
                ("foreign_keys", 1),
                ("busy_timeout", 5000),
            ):
                async with connection.execute(f"PRAGMA {name}") as cursor:
                    pragma_row = await cursor.fetchone()
                if pragma_row is None or pragma_row[0] != expected:
                    raise RuntimeError("SQLite durability configuration was not applied.")
            async with connection.execute("PRAGMA quick_check") as cursor:
                integrity = await cursor.fetchone()
            if integrity is None or integrity[0] != "ok":
                raise RuntimeError("Database integrity failed; operator action required.")
            await connection.executescript(SETTINGS_SCHEMA)
            await connection.execute(
                "INSERT INTO devices(address,name,model,created_at_ms) VALUES (?,?,?,?) "
                "ON CONFLICT(address) DO NOTHING",
                (self.core.address, self.core.name, "S300", int(time.time() * 1000)),
            )
            await connection.commit()
            async with connection.execute(
                "SELECT id FROM devices WHERE address=?", (self.core.address,)
            ) as cursor:
                row = await cursor.fetchone()
            assert row is not None
            self.device_id = int(row[0])
            async with connection.execute(
                "SELECT COUNT(*), COUNT(CASE WHEN device_id=? THEN 1 END) FROM telemetry_states",
                (self.device_id,),
            ) as cursor:
                counts = await cursor.fetchone()
            assert counts is not None
        except BaseException:
            await connection.close()
            raise
        self.db = connection
        self.state, self.error = ("ok" if self.enabled else "disabled"), None
        self.core.segment = str(uuid4())
        self.instance = hashlib.sha256(
            f"{self.path.stat().st_ino}:{self.instance}".encode()
        ).hexdigest()
        self.core.publish()
        logging.getLogger("uvicorn.error").info(
            "History database: %s; schema=%s; records=%s; current device records=%s; recording=%s",
            self.path.resolve(),
            VERSION,
            counts[0],
            counts[1],
            "enabled" if self.enabled else "disabled",
        )

    async def insert(self, observation: Observation) -> None:
        assert self.db is not None and self.device_id is not None
        sample = observation.sample
        received = int(observation.epoch * 1000)
        values = tuple(getattr(sample, field) for field in STATE_FIELDS)
        async with self.lock:
            try:
                async with self.db.execute(
                    "SELECT id,last_observed_at_ms,segment_id,"
                    + ",".join(STATE_FIELDS)
                    + " FROM telemetry_states WHERE device_id=? ORDER BY id DESC LIMIT 1",
                    (self.device_id,),
                ) as cursor:
                    previous = await cursor.fetchone()
                continuous = (
                    previous is not None
                    and previous[2] == sample.segment_id
                    and received >= previous[1]
                )
                if previous is not None and continuous and tuple(previous[3:]) == values:
                    await self.db.execute(
                        "UPDATE telemetry_states SET end_at_ms=?,last_observed_at_ms=? WHERE id=?",
                        (received + 1, received, previous[0]),
                    )
                else:
                    if previous is not None and continuous:
                        await self.db.execute(
                            "UPDATE telemetry_states SET end_at_ms=? WHERE id=?",
                            (received, previous[0]),
                        )
                    await self.db.execute(
                        INSERT_STATE,
                        (
                            self.device_id,
                            received,
                            received + 1,
                            received,
                            sample.segment_id,
                            *values,
                        ),
                    )
                await self.db.commit()
            except BaseException:
                await self.db.rollback()
                raise
        self.last_write = sample.received_at

    def schedule(self) -> None:
        latest = self.core.latest
        if (
            not self.enabled
            or self.state != "ok"
            or latest is None
            or self.core.telemetry_state() != "live"
            or latest.sample.sequence <= self.saved_sequence
        ):
            return
        # Record changes immediately; unchanged observations checkpoint coverage once
        # per UTC interval. Cached or stale values never extend the recorded span.
        bucket = math.floor(latest.epoch / self.interval)
        values = tuple(getattr(latest.sample, field) for field in STATE_FIELDS)
        if (
            latest.sample.segment_id == self.last_segment
            and values == self.saved_values
            and bucket == self.last_bucket
        ):
            return
        try:
            self.queue.put_nowait(latest)
        except asyncio.QueueFull:
            self.dropped += 1
            self.core.segment = str(uuid4())
            return
        self.saved_sequence, self.last_segment = latest.sample.sequence, latest.sample.segment_id
        self.last_bucket, self.saved_values = bucket, values

    async def run(self) -> None:
        retry = 0.0
        while not self.stopping or not self.queue.empty():
            if self.db is None and time.monotonic() >= retry and not self.stopping:
                try:
                    await self.open()
                except Exception as error:
                    self.fail(error)
                    retry = time.monotonic() + 5
            self.schedule()
            try:
                observation = await asyncio.wait_for(self.queue.get(), 0.2)
            except TimeoutError:
                continue
            try:
                await self.insert(observation)
            except Exception as error:
                self.dropped += 1
                self.fail(error)
                while not self.queue.empty():
                    self.queue.get_nowait()
                    self.queue.task_done()
                    self.dropped += 1
                if self.db:
                    async with self.lock:
                        await self.db.close()
                        self.db = None
                retry = time.monotonic() + 5
            finally:
                self.queue.task_done()

    def fail(self, error: Exception) -> None:
        self.state = "degraded"
        self.error = "Storage unavailable: " + (
            str(error) if isinstance(error, RuntimeError) else type(error).__name__
        )
        self.core.segment = str(uuid4())
        self.core.log("ERROR", "storage_failure", self.error)
        logging.getLogger("uvicorn.error").error(
            "History database unavailable: %s; %s", self.path.resolve(), self.error
        )
        self.core.publish()

    async def query(
        self,
        since: str | None = None,
        until: str | None = None,
        limit: int = 1000,
        cursor: str | None = None,
    ) -> Page:
        if not self.enabled or self.db is None or self.state != "ok":
            raise AppError("history_unavailable", "History is disabled or degraded.", 503)
        if not 1 <= limit <= 10000:
            raise AppError("invalid_limit", "History limit must be 1..10000.", 422)
        if self.queries >= 4:
            raise AppError("history_busy", "Too many history queries.", 429, True)
        self.queries += 1
        try:
            async with asyncio.timeout(6):
                async with self.lock:
                    return await self._query(since, until, limit, cursor)
        except TimeoutError:
            raise AppError(
                "query_deadline", "History query exceeded its deadline.", 503, True
            ) from None
        finally:
            self.queries -= 1

    async def _query(
        self, since: str | None, until: str | None, limit: int, cursor: str | None
    ) -> Page:
        assert self.db is not None
        now = int(self.core.clock.time() * 1000)
        start, end = aware_ms(since) if since else now - 3600000, aware_ms(until) if until else now
        after_time, after_id = -9223372036854775808, 0
        if cursor:
            data = self.codec.decode(cursor)
            if data.get("instance") != self.instance or data.get("device") != self.device_id:
                raise AppError("history_cursor_expired", "History database instance changed.", 410)
            if (since and start != data["start"]) or (until and end != data["end"]):
                raise AppError(
                    "cursor_filter_conflict", "Cursor filters differ from the first page.", 422
                )
            start, end, high = data["start"], data["end"], data["high"]
            after_time, after_id = data["time"], data["id"]
        else:
            async with self.db.execute(
                "SELECT coalesce(max(id),0) FROM telemetry_states WHERE device_id=?",
                (self.device_id,),
            ) as query:
                row = await query.fetchone()
            high = row[0] if row else 0
        if start >= end:
            raise AppError("invalid_range", "since must be before until.", 422)
        self.db.row_factory = aiosqlite.Row
        async with self.db.execute(
            "SELECT * FROM telemetry_states WHERE device_id=? AND received_at_ms>=? "
            "AND received_at_ms<? AND id<=? AND (received_at_ms>? OR (received_at_ms=? AND id>?)) "
            "ORDER BY received_at_ms,id LIMIT ?",
            (self.device_id, start, end, high, after_time, after_time, after_id, limit + 1),
        ) as query:
            rows = list(await query.fetchall())
        items = [dict(row) for row in rows[:limit]]
        next_cursor = None
        if len(rows) > limit:
            last = items[-1]
            next_cursor = self.codec.encode(
                {
                    "instance": self.instance,
                    "device": self.device_id,
                    "start": start,
                    "end": end,
                    "high": high,
                    "time": last["received_at_ms"],
                    "id": last["id"],
                }
            )
        return Page(items=items, next_cursor=next_cursor)

    async def aggregates(
        self, since: str, until: str, bucket_seconds: int = 10, limit: int = 256
    ) -> HistoryAggregates:
        if not self.enabled or self.db is None or self.state != "ok":
            raise AppError("history_unavailable", "History is disabled or degraded.", 503)
        if bucket_seconds not in (10, 30, 60, 3600) or not 1 <= limit <= 256:
            raise AppError(
                "invalid_aggregation", "Use 10, 30, 60 or 3600 seconds and limit 1..256.", 422
            )
        start, end = aware_ms(since), aware_ms(until)
        if start >= end:
            raise AppError("invalid_range", "since must be before until.", 422)
        span = bucket_seconds * 1000
        if (end - 1) // span - start // span + 1 > limit:
            raise AppError("aggregate_range_too_large", "Range exceeds the bucket limit.", 422)
        if self.queries >= 4:
            raise AppError("history_busy", "Too many history queries.", 429, True)
        self.queries += 1
        try:
            async with asyncio.timeout(6):
                async with self.lock:
                    async with self.db.execute(
                        AGGREGATES,
                        (
                            start // span * span,
                            span,
                            span,
                            end,
                            self.device_id,
                            start,
                            end,
                            span,
                            end,
                            start,
                            span,
                        ),
                    ) as query:
                        rows = await query.fetchall()
                    return HistoryAggregates(
                        bucket_seconds=cast(Literal[10, 30, 60, 3600], bucket_seconds),
                        since_ms=start,
                        until_ms=end,
                        items=[
                            HistoryBucket(
                                bucket_start_ms=row[0],
                                input_power_w=row[1],
                                output_power_w=row[2],
                                sample_count=row[3],
                            )
                            for row in rows
                        ],
                    )
        except TimeoutError:
            raise AppError(
                "query_deadline", "History query exceeded its deadline.", 503, True
            ) from None
        finally:
            self.queries -= 1

    async def settings(self, update: SettingsUpdate | None = None) -> Settings:
        if self.db is None or self.state not in {"ok", "disabled"}:
            raise AppError("settings_unavailable", "Settings storage is unavailable.", 503, True)
        try:
            async with asyncio.timeout(6):
                async with self.lock:
                    if self.db is None:
                        raise AppError(
                            "settings_unavailable", "Settings storage is unavailable.", 503, True
                        )
                    if update is not None:
                        values = update.model_dump(exclude_unset=True)
                        try:
                            await self.db.executemany(
                                "INSERT INTO settings(key,value_json) VALUES (?,?) "
                                "ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
                                [(key, json.dumps(value)) for key, value in values.items()],
                            )
                            await self.db.commit()
                        except BaseException:
                            await self.db.rollback()
                            raise
                    async with self.db.execute("SELECT key,value_json FROM settings") as cursor:
                        rows = await cursor.fetchall()
                    fields = Settings.model_fields.keys() - {"schema_version"}
                    return Settings.model_validate(
                        {key: json.loads(value) for key, value in rows if key in fields}
                    )
        except (TimeoutError, sqlite3.Error, ValueError):
            raise AppError(
                "settings_unavailable", "Settings storage is unavailable.", 503, True
            ) from None

    async def close(self) -> None:
        self.stopping = True
        if self.db:
            await self.db.close()
            self.db = None
