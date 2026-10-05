"""Bounded asynchronous telemetry persistence, without raw frames or log tables."""

import asyncio
import base64
import hashlib
import hmac
import json
import os
import sqlite3
import time
from pathlib import Path
from typing import Any
from uuid import uuid4

import aiosqlite

from mypowers.contracts import AppError, HistoryAggregates, HistoryBucket, Page, aware_ms
from mypowers.core import Core, Observation

SCHEMA = """
BEGIN IMMEDIATE;
CREATE TABLE devices (
 id INTEGER PRIMARY KEY, address TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 model TEXT NOT NULL, created_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE telemetry (
 id INTEGER PRIMARY KEY, device_id INTEGER NOT NULL REFERENCES devices(id),
 received_at_ms INTEGER NOT NULL, segment_id TEXT NOT NULL,
 battery_percent INTEGER NOT NULL CHECK (battery_percent BETWEEN 0 AND 100),
 input_power_w INTEGER NOT NULL CHECK (input_power_w BETWEEN 0 AND 65535),
 output_power_w INTEGER NOT NULL CHECK (output_power_w BETWEEN 0 AND 65535),
 remaining_minutes INTEGER NOT NULL CHECK (remaining_minutes BETWEEN 0 AND 65535),
 ac_enabled INTEGER NOT NULL CHECK (ac_enabled IN (0,1)),
 dc_enabled INTEGER NOT NULL CHECK (dc_enabled IN (0,1)),
 light_enabled INTEGER NOT NULL CHECK (light_enabled IN (0,1)),
 status_flags INTEGER NOT NULL CHECK (status_flags BETWEEN 0 AND 127),
 CHECK (ac_enabled = ((status_flags & 2) != 0)),
 CHECK (dc_enabled = ((status_flags & 1) != 0)),
 CHECK (light_enabled = ((status_flags & 16) != 0))
) STRICT;
CREATE INDEX telemetry_device_time_id ON telemetry(device_id, received_at_ms, id);
PRAGMA user_version=1;
COMMIT;
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
        self.deadline = 0.0
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
            elif version != 1:
                raise RuntimeError("Unsupported database schema; operator action required.")
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
        except BaseException:
            await connection.close()
            raise
        self.db = connection
        self.state, self.error = "ok", None
        self.core.segment = str(uuid4())
        self.instance = hashlib.sha256(
            f"{self.path.stat().st_ino}:{self.instance}".encode()
        ).hexdigest()
        self.core.publish()

    async def insert(self, observation: Observation) -> None:
        assert self.db is not None and self.device_id is not None
        sample = observation.sample
        async with self.lock:
            await self.db.execute(
                "INSERT INTO telemetry(device_id,received_at_ms,segment_id,battery_percent,"
                "input_power_w,output_power_w,remaining_minutes,ac_enabled,"
                "dc_enabled,light_enabled,status_flags) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                (
                    self.device_id,
                    int(observation.epoch * 1000),
                    sample.segment_id,
                    sample.battery_percent,
                    sample.input_power_w,
                    sample.output_power_w,
                    sample.remaining_minutes,
                    int(sample.ac_enabled),
                    int(sample.dc_enabled),
                    int(sample.light_enabled),
                    sample.status_flags,
                ),
            )
            await self.db.commit()
        self.last_write = sample.received_at

    def schedule(self) -> None:
        latest = self.core.latest
        now = self.core.clock.monotonic()
        if (
            not self.enabled
            or self.state != "ok"
            or latest is None
            or self.core.telemetry_state() != "live"
            or latest.sample.sequence <= self.saved_sequence
        ):
            return
        if latest.sample.segment_id == self.last_segment and now < self.deadline:
            return
        try:
            self.queue.put_nowait(latest)
        except asyncio.QueueFull:
            self.dropped += 1
            self.core.segment = str(uuid4())
            return
        self.saved_sequence, self.last_segment = latest.sample.sequence, latest.sample.segment_id
        self.deadline = now + self.interval

    async def run(self) -> None:
        retry = 0.0
        while not self.stopping or not self.queue.empty():
            if self.enabled and self.db is None and time.monotonic() >= retry and not self.stopping:
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
                "SELECT coalesce(max(id),0) FROM telemetry WHERE device_id=?", (self.device_id,)
            ) as query:
                row = await query.fetchone()
            high = row[0] if row else 0
        if start >= end:
            raise AppError("invalid_range", "since must be before until.", 422)
        self.db.row_factory = aiosqlite.Row
        async with self.db.execute(
            "SELECT * FROM telemetry WHERE device_id=? AND received_at_ms>=? "
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
        if bucket_seconds not in (10, 60, 3600) or not 1 <= limit <= 256:
            raise AppError(
                "invalid_aggregation", "Use 10, 60 or 3600 seconds and limit 1..256.", 422
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
                    # UTC epoch buckets, including correct floor division before 1970.
                    # AVG includes measured zeros; absent buckets have no returned record.
                    async with self.db.execute(
                        "SELECT received_at_ms - ((received_at_ms % ? + ?) % ?) "
                        "AS bucket_start_ms, AVG(input_power_w), AVG(output_power_w), COUNT(*) "
                        "FROM telemetry WHERE device_id=? AND received_at_ms>=? "
                        "AND received_at_ms<? GROUP BY bucket_start_ms ORDER BY bucket_start_ms",
                        (span, span, span, self.device_id, start, end),
                    ) as query:
                        rows = await query.fetchall()
                    return HistoryAggregates(
                        bucket_seconds=bucket_seconds,
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

    async def close(self) -> None:
        self.stopping = True
        if self.db:
            await self.db.close()
            self.db = None
