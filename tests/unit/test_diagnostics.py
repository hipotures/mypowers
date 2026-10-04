import asyncio
import json
import logging

import pytest

from mypowers.contracts import AppError, Level
from mypowers.diagnostics import Diagnostics, plain


async def flush(logs):
    await asyncio.to_thread(logs.queue.join)


async def test_jsonl_redaction_debug_levels_expiry_and_rotation(tmp_path):
    secret = "credential-content-" * 3
    logs = Diagnostics(
        "instance", tmp_path, "WARNING", max_bytes=1024, backups=3, secrets=(secret,)
    )
    logs.start()
    try:
        logs.log("INFO", "ignored", "Do not enqueue baseline INFO.")
        logs.log(
            "WARNING",
            "event",
            "Newline\n" + secret + "\x1b[31m",
            token=secret,
            authorization="Bearer " + secret,
            frame_hex="0000",
            unsafe="http://user:pw@host?token=value",
        )
        assert logs.sequence == 1
        health = logs.set_level(Level.DEBUG, 0.01)
        assert health["configured_level"] == "WARNING" and health["effective_level"] == "DEBUG"
        logs.log("DEBUG", "raw", "BLE capture", frame_hex="a565")
        await flush(logs)
        data = (tmp_path / "mypowers.jsonl").read_text()
        assert secret not in data and "\x1b" not in data
        records = [json.loads(line) for line in data.splitlines()]
        event = next(record for record in records if record["event"] == "event")
        assert event["context"]["token"] == "[redacted]"
        assert "frame_hex" not in event["context"]
        assert any(r["context"].get("frame_hex") == "a565" for r in records)
        await asyncio.sleep(0.02)
        assert logs.health()["effective_level"] == "WARNING"
        logs.set_level(Level.DEBUG)
        for index in range(20):
            logs.log("INFO", "row", "message-" * 10, index=index)
        await flush(logs)
        assert (tmp_path / "mypowers.jsonl.1").exists()
        page = await logs.query(tail=10)
        assert len(page.items) == 10
        assert all(r["event"] == "row" for r in page.items)
        logs.set_level(None)
        assert logs.effective == "WARNING"
    finally:
        await logs.close()


async def test_log_pagination_rotation_expiry_malformed_final_line(tmp_path):
    logs = Diagnostics("instance", tmp_path, max_bytes=1024, backups=1)
    logs.start()
    try:
        for index in range(3):
            logs.log("INFO", "row", "record", index=index)
        await flush(logs)
        page = await logs.query(limit=1)
        second = await logs.query(limit=10, cursor=page.next_cursor)
        assert [r["context"]["index"] for r in page.items + second.items] == [0, 1, 2]
        with pytest.raises(AppError, match="filters"):
            await logs.query(cursor=page.next_cursor, min_level=Level.ERROR)
        for index in range(40):
            logs.log("INFO", "row", "rotation" * 20, index=index)
        await flush(logs)
        with pytest.raises(AppError) as caught:
            await logs.query(cursor=page.next_cursor)
        assert caught.value.status == 410
        with (tmp_path / "mypowers.jsonl").open("a") as file:
            file.write('not-json\n{"partial":')
        page = await logs.query(tail=10)
        assert page.skipped_lines >= 2
        with pytest.raises(AppError):
            await logs.query(tail=1, since="2026-10-04T00:00:00Z")
        with pytest.raises(AppError):
            await logs.query(limit=1001)
        logs.queries = 4
        with pytest.raises(AppError):
            await logs.query()
    finally:
        await logs.close()


async def test_file_failure_ring_and_queue_bounds(tmp_path):
    file = tmp_path / "not-a-directory"
    file.write_text("occupied")
    logs = Diagnostics("instance", file)
    logs.start()
    try:
        logs.log("ERROR", "sink", "file failure")
        await flush(logs)
        assert logs.health()["state"] == "degraded"
        page = await logs.query(tail=10)
        assert page.source == "ring" and page.gap
        sub = logs.subscribe()
        for index in range(300):
            logs.publish({"event": "event", "index": index})
        assert sub.qsize() <= 256
        for index in range(16):
            if index:
                logs.subscribe()
        with pytest.raises(AppError):
            logs.subscribe()
        logs.subscribers.clear()
        assert plain("a\x1bb\x00c") == "abc"
        assert (
            logs.redact({"cookie": "secret", "nested": {"password": "secret"}})["cookie"]
            == "[redacted]"
        )
    finally:
        await logs.close()


async def test_bounded_record_rate_limit_and_non_debug_no_raw(tmp_path):
    logs = Diagnostics("instance", tmp_path)
    logs.start()
    try:
        logs.log("ERROR", "same", "first")
        logs.log("ERROR", "same", "duplicate")
        assert logs.sequence == 1
        logs.log("INFO", "oversize", "x" * 10000, **{f"key{n}": "y" * 4096 for n in range(64)})
        await flush(logs)
        records = [
            json.loads(line) for line in (tmp_path / "mypowers.jsonl").read_text().splitlines()
        ]
        assert records[-1]["context"] == {"truncated": True}
        assert len(json.dumps(records[-1])) < 16384
    finally:
        await logs.close()


def test_supervisor_redaction_including_exception_and_query_token():
    from mypowers.diagnostics.redaction import RedactionFilter

    secret = "secret-material-" * 3
    record = logging.LogRecord(
        "uvicorn.error",
        logging.WARNING,
        "",
        0,
        "/api/v1/events?token=" + secret + " Bearer " + secret,
        (),
        None,
    )
    assert RedactionFilter((secret,)).filter(record)
    assert secret not in record.getMessage()
    try:
        raise ValueError("https://user:password@host?token=" + secret)
    except ValueError:
        import sys

        record.exc_info = sys.exc_info()
    assert RedactionFilter((secret,)).filter(record)
    assert record.exc_info is None and secret not in record.getMessage()


def test_overload_prioritizes_operational_records_over_debug(tmp_path):
    logs = Diagnostics("instance", tmp_path)
    logs.accept("INFO", "important", "Retain this operational event.", {})
    for _ in range(2047):
        logs.accept("DEBUG", "traffic", "Bounded debug traffic.", {})
    logs.accept("INFO", "command", "Keep command outcome when debug fills the queue.", {})
    assert logs.queue.qsize() == 2048 and logs.dropped == 1
    records = []
    while not logs.queue.empty():
        records.append(logs.queue.get_nowait())
        logs.queue.task_done()
    logs.queue.join()
    assert records[0]["event"] == "important"
    assert records[-1]["event"] == "command"
    assert sum(record["level"] == "DEBUG" for record in records) == 2046
