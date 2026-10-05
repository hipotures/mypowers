"""Bounded, redacted standard-logging records and application-owned log queries."""

import asyncio
import json
import logging
import logging.handlers
import queue
import re
import sys
import threading
import time
from collections import deque
from collections.abc import Iterator
from contextlib import ExitStack
from pathlib import Path
from typing import Any, BinaryIO, Literal

from mypowers.contracts import AppError, Level, LogPage, aware_ms, timestamp, utc_now
from mypowers.storage import CursorCodec

SECRET_KEYS = re.compile(r"token|authorization|cookie|password|credential|secret|dotenv", re.I)
CONTROLS = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")


def plain(value: str) -> str:
    return CONTROLS.sub("", value)


class StructuredHandler(logging.Handler):
    def __init__(self, owner: "Diagnostics"):
        super().__init__()
        self.owner = owner

    def emit(self, record: logging.LogRecord) -> None:
        owner = self.owner
        context = getattr(record, "context", {})
        owner.accept(
            record.levelname, getattr(record, "event", "application"), record.getMessage(), context
        )


class Diagnostics:
    def __init__(
        self,
        instance: str,
        directory: Path,
        baseline: str = "INFO",
        max_bytes: int = 10485760,
        backups: int = 5,
        secrets: tuple[str, ...] = (),
    ):
        self.instance, self.directory = instance, directory
        self.baseline, self.effective = baseline, baseline
        self.expires: float | None = None
        self.expiry_epoch: float | None = None
        self.max_bytes, self.backups, self.secrets = max_bytes, backups, secrets
        self.records: deque[dict[str, Any]] = deque(maxlen=1000)
        self.mutex = threading.Lock()
        self.queue: queue.Queue[dict[str, Any] | None] = queue.Queue(2048)
        self.subscribers: set[asyncio.Queue[dict[str, Any]]] = set()
        self.sequence = self.dropped = self.malformed = 0
        self.state = "starting"
        self.thread: threading.Thread | None = None
        self.loop: asyncio.AbstractEventLoop | None = None
        self.handler = StructuredHandler(self)
        self.logger = logging.getLogger("mypowers")
        self.codec = CursorCodec()
        self.queries = 0
        self.error_times: dict[str, float] = {}

    def redact(self, value: Any, key: str = "", depth: int = 0) -> Any:
        if SECRET_KEYS.search(key):
            return "[redacted]"
        if depth > 5:
            return "[truncated]"
        if isinstance(value, dict):
            return {
                str(k)[:128]: self.redact(v, str(k), depth + 1) for k, v in list(value.items())[:64]
            }
        if isinstance(value, (tuple, list)):
            return [self.redact(v, depth=depth + 1) for v in value[:64]]
        if isinstance(value, str):
            for secret in self.secrets:
                if secret:
                    value = value.replace(secret, "[redacted]")
            value = re.sub(r"(?i)bearer\s+\S+", "Bearer [redacted]", value)
            value = re.sub(r"https?://[^\s]+", "[url redacted]", value)
            return plain(value)[:4096]
        if value is None or isinstance(value, (bool, int, float)):
            return value
        return self.redact(str(value), depth=depth + 1)

    def start(self) -> None:
        self.loop = asyncio.get_running_loop()
        self.logger.setLevel(getattr(logging, self.effective))
        self.logger.addHandler(self.handler)
        self.logger.propagate = False
        self.thread = threading.Thread(target=self.writer, name="mypowers-jsonl", daemon=True)
        self.thread.start()

    def log(self, level: str, event: str, message: str, **context: Any) -> None:
        self.logger.log(
            getattr(logging, level), message, extra={"event": event, "context": context}
        )

    def accept(self, level: str, event: str, message: str, context: dict[str, Any]) -> None:
        now = time.monotonic()
        if level in {"WARNING", "ERROR"}:
            with self.mutex:
                if now - self.error_times.get(event, -100) < 5:
                    return
                if len(self.error_times) >= 128:
                    self.error_times.clear()
                self.error_times[event] = now
        if level != "DEBUG":
            context = {k: v for k, v in context.items() if k != "frame_hex"}
        with self.mutex:
            self.sequence += 1
            record = {
                "schema_version": 1,
                "timestamp": utc_now(),
                "server_instance_id": self.instance,
                "sequence": self.sequence,
                "level": level,
                "logger": "mypowers",
                "event": event,
                "message": self.redact(message),
                "context": self.redact(context),
            }
            encoded = json.dumps(record, ensure_ascii=True)
            if len(encoded.encode()) > 16384:
                record["context"] = {"truncated": True}
            self.records.append(record)
        try:
            self.queue.put_nowait(record)
        except queue.Full:
            self.dropped += 1
            if level != "DEBUG":
                # Replace DEBUG under the Queue lock; preserve FIFO order and join accounting.
                with self.queue.mutex:
                    pending: deque[dict[str, Any] | None] = self.queue.queue
                    debug_index = next(
                        (
                            index
                            for index, item in enumerate(pending)
                            if item is not None and item.get("level") == "DEBUG"
                        ),
                        None,
                    )
                    if debug_index is not None:
                        del pending[debug_index]
                        pending.append(record)
                        self.queue.not_empty.notify()
        if self.loop and not self.loop.is_closed():
            self.loop.call_soon_threadsafe(self.publish, record)

    def publish(self, record: dict[str, Any]) -> None:
        for subscriber in self.subscribers:
            if subscriber.full():
                while not subscriber.empty():
                    subscriber.get_nowait()
                subscriber.put_nowait({"event": "stream_overflow"})
            else:
                subscriber.put_nowait(record)

    def subscribe(self) -> asyncio.Queue[dict[str, Any]]:
        if len(self.subscribers) >= 16:
            raise AppError("stream_limit", "Too many log streams.", 429)
        subscriber: asyncio.Queue[dict[str, Any]] = asyncio.Queue(256)
        self.subscribers.add(subscriber)
        return subscriber

    def writer(self) -> None:
        sink: logging.handlers.RotatingFileHandler | None = None
        while True:
            item = self.queue.get()
            try:
                if item is None:
                    if sink:
                        sink.close()
                    return
                if sink is None:
                    try:
                        self.directory.mkdir(parents=True, exist_ok=True)
                        sink = logging.handlers.RotatingFileHandler(
                            self.directory / "mypowers.jsonl",
                            maxBytes=self.max_bytes,
                            backupCount=self.backups,
                            encoding="utf-8",
                        )
                        self.state = "ok"
                    except OSError:
                        self.state = "degraded"
                encoded = json.dumps(item, ensure_ascii=True, separators=(",", ":"))
                if sink:
                    try:
                        record = logging.LogRecord(
                            "mypowers", logging.INFO, "", 0, encoded, (), None
                        )
                        if sink.shouldRollover(record):
                            sink.doRollover()
                        assert sink.stream is not None
                        sink.stream.write(encoded + "\n")
                        sink.flush()
                    except OSError:
                        self.state = "degraded"
                        sink.close()
                        sink = None
                        print(encoded, file=sys.stderr)
                else:
                    print(encoded, file=sys.stderr)
            finally:
                self.queue.task_done()
        # The sentinel closes the sink in close(), after the writer exits.

    def tick(self) -> None:
        if self.expires is not None and time.monotonic() >= self.expires:
            self.set_level(None)

    def health(self) -> dict[str, Any]:
        self.tick()
        return {
            "state": self.state,
            "configured_level": self.baseline,
            "effective_level": self.effective,
            "override_expires_at": timestamp(self.expiry_epoch)
            if self.expiry_epoch is not None
            else None,
            "log_records_dropped": self.dropped,
            "pending_count": self.queue.qsize(),
            "source": "files" if self.state == "ok" else "ring",
            "lost_history": self.state == "degraded",
        }

    def set_level(self, level: Level | None, duration: float | None = None) -> dict[str, Any]:
        self.effective = level.value if level else self.baseline
        self.expires = time.monotonic() + duration if duration is not None else None
        self.expiry_epoch = time.time() + duration if duration is not None else None
        self.logger.setLevel(getattr(logging, self.effective))
        # Record the setting change even when the new level suppresses ordinary INFO messages.
        self.accept(
            "INFO", "log_level_changed", "Runtime log level changed.", {"effective": self.effective}
        )
        return self.health()

    async def query(
        self,
        *,
        tail: int | None = None,
        since: str | None = None,
        until: str | None = None,
        min_level: Level = Level.DEBUG,
        limit: int = 100,
        cursor: str | None = None,
        direction: Literal["forward", "backward"] = "forward",
    ) -> LogPage:
        if tail is not None and (since or until or cursor):
            raise AppError("invalid_log_query", "tail and range/cursor modes are exclusive.", 422)
        if not 1 <= limit <= 1000 or tail is not None and not 1 <= tail <= 1000:
            raise AppError("invalid_limit", "Log limit and tail must be 1..1000.", 422)
        start = aware_ms(since) if since else None
        end = aware_ms(until) if until else None
        if start is not None and end is not None and start >= end:
            raise AppError("invalid_range", "since must be before until.", 422)
        if self.queries >= 4:
            raise AppError("logs_busy", "Too many log queries.", 429)
        filters = {"start": start, "end": end, "level": min_level.value}
        decoded = self.codec.decode(cursor) if cursor else None
        if decoded:
            retained_filters = decoded.get("filters", {})
            if (
                (since and retained_filters.get("start") != start)
                or (until and retained_filters.get("end") != end)
                or retained_filters.get("level") != min_level.value
            ):
                raise AppError("cursor_filter_conflict", "Cursor filters changed.", 422)
            filters = retained_filters
        self.queries += 1
        try:
            return await asyncio.to_thread(self.scan, tail, filters, limit, decoded, direction)
        finally:
            self.queries -= 1

    def matches(self, record: dict[str, Any], filters: dict[str, Any]) -> bool:
        try:
            stamp = aware_ms(record["timestamp"])
            return (
                getattr(logging, record["level"]) >= getattr(logging, filters["level"])
                and (filters["start"] is None or stamp >= filters["start"])
                and (filters["end"] is None or stamp < filters["end"])
            )
        except (KeyError, AttributeError, AppError, TypeError):
            return False

    def scan(
        self,
        tail: int | None,
        filters: dict[str, Any],
        limit: int,
        decoded: dict[str, Any] | None,
        direction: Literal["forward", "backward"],
    ) -> LogPage:
        backward = tail is not None or direction == "backward"
        count = tail or limit
        if self.state != "ok":
            with self.mutex:
                retained = list(self.records)
            if decoded and (
                decoded.get("source") != "ring"
                or not retained
                or decoded.get("sequence", 0) < retained[0]["sequence"] - 1
            ):
                raise AppError("log_cursor_expired", "Ring cursor expired.", 410)
            selected = [
                r
                for r in retained
                if self.matches(r, filters)
                and (
                    not decoded
                    or (
                        r["sequence"] <= decoded["sequence"]
                        if backward
                        else r["sequence"] > decoded["sequence"]
                    )
                )
            ]
            ring_items = selected[-count:] if backward else selected[:count]
            next_cursor = (
                self.codec.encode(
                    {"source": "ring", "sequence": ring_items[-1]["sequence"], "filters": filters}
                )
                if ring_items
                else None
            )
            previous_cursor = (
                self.codec.encode(
                    {
                        "source": "ring",
                        "sequence": ring_items[0]["sequence"] - 1,
                        "filters": filters,
                    }
                )
                if ring_items
                else None
            )
            return LogPage(
                items=ring_items,
                next_cursor=next_cursor,
                previous_cursor=previous_cursor,
                has_more_before=len(selected) > count if backward else bool(decoded),
                has_more_after=bool(decoded) if backward else len(selected) > count,
                source="ring",
                gap=True,
            )
        paths = [self.directory / f"mypowers.jsonl.{n}" for n in range(self.backups, 0, -1)]
        paths.append(self.directory / "mypowers.jsonl")
        deadline, scanned, skipped = time.monotonic() + 3, 0, 0
        rows: list[tuple[dict[str, Any], int, int, int]] = []

        def charge(size: int) -> None:
            nonlocal scanned
            scanned += size
            if scanned > 67108864 or time.monotonic() > deadline:
                raise AppError(
                    "log_scan_budget", "Log query exceeded scan budget; narrow the range.", 503
                )

        def lines(file: BinaryIO, start: int, end: int) -> Iterator[tuple[int, int, bytes]]:
            if not backward:
                file.seek(start)
                while file.tell() < end:
                    offset = file.tell()
                    raw = file.readline(min(16385, end - offset))
                    charge(len(raw))
                    yield offset, file.tell(), raw
                return
            # Locate line boundaries in bounded chunks, reading only one bounded record at a time.
            position, record_end = end, end
            while position > start:
                chunk_start = max(start, position - 65536)
                file.seek(chunk_start)
                chunk = file.read(position - chunk_start)
                charge(len(chunk))
                position = chunk_start
                for index in range(len(chunk) - 1, -1, -1):
                    if chunk[index] != 10:
                        continue
                    offset = chunk_start + index + 1
                    if offset >= record_end:
                        continue
                    file.seek(offset)
                    raw = file.read(min(16385, record_end - offset))
                    yield offset, record_end, raw
                    record_end = offset
            if record_end > start:
                file.seek(start)
                yield start, record_end, file.read(min(16385, record_end - start))

        try:
            with ExitStack() as stack:
                files: list[tuple[BinaryIO, int, int]] = []
                for path in paths:
                    try:
                        opened = stack.enter_context(path.open("rb"))
                    except FileNotFoundError:
                        continue
                    stat = __import__("os").fstat(opened.fileno())
                    files.append((opened, stat.st_ino, stat.st_size))
                boundary = None
                if decoded:
                    boundary = next(
                        (
                            i
                            for i, (_, inode, _) in enumerate(files)
                            if inode == decoded.get("inode")
                        ),
                        None,
                    )
                    if decoded.get("source") != "files" or boundary is None:
                        raise AppError(
                            "log_cursor_expired", "Rotation removed the cursor source.", 410
                        )
                    if decoded["offset"] > files[boundary][2]:
                        raise AppError("log_cursor_expired", "Cursor source was truncated.", 410)
                indices = range(len(files) - 1, -1, -1) if backward else range(len(files))
                for index in indices:
                    if boundary is not None and (
                        index > boundary if backward else index < boundary
                    ):
                        continue
                    file, inode, size = files[index]
                    offset = decoded["offset"] if decoded and index == boundary else None
                    start = offset if offset is not None and not backward else 0
                    end = offset if offset is not None and backward else size
                    for before, after, raw in lines(file, start, end):
                        if len(raw) > 16384 or not raw.endswith(b"\n"):
                            skipped += 1
                            continue
                        try:
                            record = json.loads(raw)
                            if not isinstance(record, dict) or not self.matches(record, filters):
                                continue
                        except (ValueError, UnicodeDecodeError):
                            skipped += 1
                            continue
                        rows.append((record, inode, before, after))
                        if len(rows) > count:
                            break
                    if len(rows) > count:
                        break
        except OSError:
            raise AppError(
                "log_files_unavailable", "Application log files are unavailable.", 503
            ) from None
        more = len(rows) > count
        rows = rows[:count]
        if backward:
            rows.reverse()

        def cursor(row: tuple[dict[str, Any], int, int, int], before: bool) -> str:
            return self.codec.encode(
                {
                    "source": "files",
                    "inode": row[1],
                    "offset": row[2 if before else 3],
                    "filters": filters,
                }
            )

        return LogPage(
            items=[row[0] for row in rows],
            next_cursor=cursor(rows[-1], False) if rows else None,
            previous_cursor=cursor(rows[0], True) if rows else None,
            has_more_before=more if backward else bool(decoded),
            has_more_after=bool(decoded) if backward else more,
            source="files",
            skipped_lines=skipped,
        )

    async def close(self) -> None:
        self.logger.removeHandler(self.handler)
        try:
            await asyncio.wait_for(asyncio.to_thread(self.queue.put, None, True, 1), 2)
        except (TimeoutError, queue.Full):
            self.dropped += self.queue.qsize()
        if self.thread:
            await asyncio.to_thread(self.thread.join, 2)
