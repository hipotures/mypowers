import asyncio
import json
import logging

import pytest

from mypowers.contracts import AppError, Level
from mypowers.diagnostics import Diagnostics, plain


async def flush(logs):
    await asyncio.to_thread(logs.queue.join)


async def test_bidirectional_day_pages_across_rotated_files_and_new_records(tmp_path):
    logs = Diagnostics("instance", tmp_path, backups=2)
    logs.state = "ok"
    records = [
        {
            "timestamp": f"2026-10-0{4 if index < 70 else 5}T12:00:{index % 60:02}Z",
            "sequence": index,
            "level": "INFO" if index % 2 == 0 else "DEBUG",
            "message": f"Record {index} — Unicode",
        }
        for index in range(90)
    ]
    for name, selected in [
        ("mypowers.jsonl.2", records[:25]),
        ("mypowers.jsonl.1", records[25:50]),
        ("mypowers.jsonl", records[50:]),
    ]:
        (tmp_path / name).write_text(
            "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in selected)
        )
    arguments = {"since": "2026-10-04T00:00:00Z", "until": "2026-10-05T00:00:00Z", "limit": 7}
    latest = await logs.query(**arguments, direction="backward")
    assert [row["sequence"] for row in latest.items] == list(range(63, 70))
    assert latest.has_more_before and not latest.has_more_after
    older = await logs.query(**arguments, direction="backward", cursor=latest.previous_cursor)
    assert [row["sequence"] for row in older.items] == list(range(56, 63))
    newer = await logs.query(**arguments, cursor=older.next_cursor)
    assert newer.items == latest.items
    collected = list(latest.items)
    page = latest
    while page.has_more_before:
        page = await logs.query(**arguments, direction="backward", cursor=page.previous_cursor)
        assert len(page.items) <= 7
        collected[:0] = page.items
    assert [row["sequence"] for row in collected] == list(range(70))
    first = await logs.query(**arguments)
    assert not first.has_more_before and first.has_more_after
    filtered = await logs.query(**arguments, direction="backward", min_level=Level.INFO)
    assert all(row["level"] == "INFO" for row in filtered.items)
    with pytest.raises(AppError, match="filters"):
        await logs.query(
            **{**arguments, "until": "2026-10-06T00:00:00Z"}, cursor=latest.next_cursor
        )
    with (tmp_path / "mypowers.jsonl").open("a") as file:
        file.write(json.dumps({**records[0], "sequence": 90}) + "\n")
    delta = await logs.query(**arguments, cursor=latest.next_cursor)
    assert [row["sequence"] for row in delta.items] == [90]
    stable_older = await logs.query(
        **arguments, direction="backward", cursor=latest.previous_cursor
    )
    assert stable_older.items == older.items


async def test_backward_scan_handles_chunk_boundaries_partial_lines_and_truncation(tmp_path):
    logs = Diagnostics("instance", tmp_path)
    logs.state = "ok"
    rows = [
        {"timestamp": "2026-10-04T12:00:00Z", "level": "INFO", "message": "x" * 9000, "sequence": i}
        for i in range(30)
    ]
    path = tmp_path / "mypowers.jsonl"
    path.write_text(
        "".join(json.dumps(row) + "\n" for row in rows) + "x" * 20000 + "\nnot-json\n{partial"
    )
    page = await logs.query(limit=30, direction="backward")
    assert [row["sequence"] for row in page.items] == list(range(30))
    assert page.skipped_lines == 3
    assert not page.has_more_before
    path.write_text("")
    with pytest.raises(AppError) as caught:
        await logs.query(cursor=page.next_cursor)
    assert caught.value.status == 410


async def test_bidirectional_ring_pages_report_incomplete_history(tmp_path):
    logs = Diagnostics("instance", tmp_path)
    logs.state = "degraded"
    logs.records.extend(
        {"timestamp": "2026-10-04T12:00:00Z", "level": "INFO", "sequence": i} for i in range(1, 10)
    )
    latest = await logs.query(direction="backward", limit=3)
    older = await logs.query(direction="backward", limit=3, cursor=latest.previous_cursor)
    newer = await logs.query(limit=3, cursor=older.next_cursor)
    assert [row["sequence"] for row in latest.items] == [7, 8, 9]
    assert [row["sequence"] for row in older.items] == [4, 5, 6]
    assert newer.items == latest.items
    assert latest.gap and latest.source == "ring"


async def test_log_level_changes_are_info_audit_records_even_when_info_is_disabled(tmp_path):
    logs = Diagnostics("instance", tmp_path)
    logs.start()
    try:
        logs.set_level(Level.ERROR)
        logs.set_level(Level.INFO)
        await flush(logs)
        page = await logs.query(tail=10)
        changes = [row for row in page.items if row["event"] == "log_level_changed"]
        assert [row["level"] for row in changes] == ["INFO", "INFO"]
        assert [row["context"]["effective"] for row in changes] == ["ERROR", "INFO"]
    finally:
        await logs.close()


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


@pytest.mark.parametrize(
    ("message", "event", "context", "truncated"),
    [
        ("Station connected", "application", {"attempt": 1}, False),
        ("x" * 4096, "application", {}, False),
        ("🔋" * 4096, "application", {}, True),
        ("電" * 4096, "application", {}, True),
        ('\\"' * 2048, "application", {"payload": "🔋" * 4096}, True),
        ("Station connected", "🔋" * 4096, {}, True),
        ("🔋" * 4096, "🔋" * 4096, {}, True),
    ],
    ids=["normal", "ascii", "emoji", "cjk", "escaped-context", "large-event", "both-large"],
)
async def test_bounded_encoded_records_remain_available_in_files_ring_and_stream(
    tmp_path, message, event, context, truncated
):
    logs = Diagnostics("11111111-1111-4111-8111-111111111111", tmp_path)
    logs.start()
    subscriber = logs.subscribe()
    try:
        logs.accept("CRITICAL", event, message, context)
        await flush(logs)
        record = logs.records[-1]
        raw = (tmp_path / "mypowers.jsonl").read_bytes()
        assert len(raw) <= 16384
        assert raw.endswith(b"\n") and json.loads(raw) == record
        assert bool(record["context"].get("truncated")) == truncated
        if not truncated:
            assert record["message"] == message and record["event"] == event
            assert record["context"] == context
        else:
            assert message.startswith(record["message"])
            assert event.startswith(record["event"])
            if event == "application":
                assert record["event"] == event and record["message"]
        page = await logs.query(tail=10)
        assert page.source == "files" and page.skipped_lines == 0 and page.items == [record]
        logs.state = "degraded"
        page = await logs.query(tail=10)
        assert page.source == "ring" and page.items == [record]
        streamed = await asyncio.wait_for(subscriber.get(), 1)
        assert streamed == record
        envelope = {"type": "log", "data": streamed}
        assert len(json.dumps(envelope, ensure_ascii=False).encode("utf-8")) < 32768
    finally:
        logs.subscribers.clear()
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


@pytest.mark.parametrize("colors", [False, True])
def test_uvicorn_formatter_keeps_interpolation_and_redaction(colors):
    from uvicorn.logging import DefaultFormatter

    from mypowers.diagnostics.redaction import RedactionFilter

    formatter = DefaultFormatter(fmt="%(levelprefix)s %(message)s", use_colors=colors)
    cases = [
        ("Started server process [%d]", (4321,), "Started server process [4321]"),
        (
            "Uvicorn running on %s://%s:%d (Press CTRL+C to quit)",
            ("http", "127.0.0.1", 5364),
            "Uvicorn running on http://127.0.0.1:5364 (Press CTRL+C to quit)",
        ),
        ("Listener %s", ("http://[::1]:5364",), "Listener http://[::1]:5364"),
        ("Failed %s", ("https://user:private@host:443",), "Failed [url redacted]"),
        ("Failed %s", ("https://host:443/path?token=private",), "Failed [url redacted]"),
    ]
    for message, args, expected in cases:
        record = logging.LogRecord("uvicorn.error", logging.INFO, "", 0, message, args, None)
        record.color_message = "\x1b[36m" + message + " private\x1b[0m"
        assert RedactionFilter(("private",)).filter(record)
        rendered = formatter.format(record)
        assert expected in rendered
        assert "private" not in rendered and "%d" not in rendered and "%s" not in rendered


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
