import asyncio
import sqlite3
from contextlib import closing
from dataclasses import replace
from uuid import uuid4

import pytest
from conftest import frame

from mypowers.contracts import AppError, SettingsUpdate, timestamp
from mypowers.storage import CursorCodec, HistoryStore


async def test_schema_constraints_durability_and_colliding_times(core, tmp_path):
    service, _, _, _ = core
    store = HistoryStore(service, tmp_path, True, 10)
    await store.open()
    try:
        for _ in range(3):
            await store.insert(service.latest)
        page = await store.query(
            timestamp(service.clock.time() - 1), timestamp(service.clock.time() + 1)
        )
        assert len(page.items) == 3
        assert len({r["id"] for r in page.items}) == 3
        assert all(r["input_power_w"] == 0 for r in page.items)
        async with store.db.execute("PRAGMA table_info(telemetry)") as query:
            columns = [r[1] for r in await query.fetchall()]
        assert not {"frame", "raw", "log"} & set(columns)
        with pytest.raises(sqlite3.IntegrityError):
            await store.db.execute("UPDATE telemetry SET battery_percent=101")
        await store.db.rollback()
        with pytest.raises(sqlite3.IntegrityError):
            await store.db.execute("UPDATE telemetry SET ac_enabled=1")
        await store.db.rollback()
        async with store.db.execute("PRAGMA integrity_check") as query:
            assert (await query.fetchone())[0] == "ok"
    finally:
        await store.close()


async def test_periodic_new_samples_segments_and_no_restamping(core, tmp_path):
    service, clock, _, session = core
    store = HistoryStore(service, tmp_path, True, 10)
    await store.open()
    try:
        store.schedule()
        assert store.queue.qsize() == 1
        store.queue.get_nowait()
        store.schedule()
        assert store.queue.empty()
        clock.advance(11)
        store.schedule()
        assert store.queue.empty()  # cached/stale sample cannot be re-stamped
        service.receive(frame(), session)
        store.schedule()
        assert store.queue.qsize() == 1
        store.queue.get_nowait()
        clock.advance(0.1)
        service.receive(frame(), session)
        store.schedule()
        assert store.queue.empty()
        service.invalidate()
        service.receive(frame(), session)
        store.schedule()
        assert store.queue.qsize() == 1
    finally:
        await store.close()


@pytest.mark.parametrize("seconds", [10, 60])
async def test_periodic_history_fills_utc_buckets_without_scheduler_drift(core, tmp_path, seconds):
    service, clock, _, session = core
    store = HistoryStore(service, tmp_path, True, seconds)
    await store.open()
    start = int(clock.time() // seconds) * seconds
    try:
        for index in range(800):
            service.receive(frame(input_w=12, output_w=25), session)
            store.schedule()
            while not store.queue.empty():
                observation = store.queue.get_nowait()
                await store.insert(observation)
                store.queue.task_done()
            clock.advance((0.7, 1.3, 1.9)[index % 3])
        end = (int(service.latest.epoch // seconds) + 1) * seconds
        page = await store.aggregates(timestamp(start), timestamp(end), seconds, 256)
        assert [point.bucket_start_ms for point in page.items] == list(
            range(start * 1000, end * 1000, seconds * 1000)
        )
        assert all(point.sample_count == 1 for point in page.items)
        assert all(point.input_power_w == 12 and point.output_power_w == 25 for point in page.items)
        # A real telemetry outage still leaves empty buckets; never fill cached samples.
        clock.advance(seconds * 4)
        store.schedule()
        assert store.queue.empty()
        service.receive(frame(input_w=0, output_w=0), session)
        store.schedule()
        assert store.queue.qsize() == 1
        observation = store.queue.get_nowait()
        await store.insert(observation)
        store.queue.task_done()
        new_end = (int(observation.epoch // seconds) + 1) * seconds
        after = await store.aggregates(timestamp(end), timestamp(new_end), seconds, 8)
        assert len(after.items) == 1
        assert after.items[0].bucket_start_ms == int(observation.epoch // seconds) * seconds * 1000
        assert after.items[0].input_power_w == after.items[0].output_power_w == 0
    finally:
        await store.close()


async def test_stable_high_water_pagination_under_backwards_insertion(core, tmp_path):
    service, _, _, _ = core
    store = HistoryStore(service, tmp_path, True, 10)
    await store.open()
    try:
        now = service.latest.epoch
        for delta in (0, 0, 1, 2):
            await store.insert(replace(service.latest, epoch=now + delta))
        start, end = timestamp(now - 1), timestamp(now + 10)
        first = await store.query(start, end, 2)
        assert len(first.items) == 2 and first.next_cursor
        await store.insert(replace(service.latest, epoch=now - 0.1))
        second = await store.query(start, end, 2, first.next_cursor)
        assert [r["id"] for r in first.items + second.items] == [1, 2, 3, 4]
        with pytest.raises(AppError, match="filters"):
            await store.query(timestamp(now - 2), end, 2, first.next_cursor)
        store.instance = str(uuid4())
        with pytest.raises(AppError) as caught:
            await store.query(start, end, 2, first.next_cursor)
        assert caught.value.status == 410
        assert (await store.query(timestamp(now + 20), timestamp(now + 30))).items == []
        with pytest.raises(AppError):
            await store.query(end, start)
        with pytest.raises(AppError):
            await store.query(limit=10001)
        store.queries = 4
        with pytest.raises(AppError):
            await store.query(start, end)
    finally:
        await store.close()


@pytest.mark.parametrize("kind", ["future", "unversioned", "corrupt"])
async def test_unknown_database_never_overwritten(core, tmp_path, kind):
    store = HistoryStore(core[0], tmp_path, True, 10)
    if kind == "corrupt":
        store.path.write_bytes(b"corrupt SQLite content")
    else:
        with closing(sqlite3.connect(store.path)) as connection:
            connection.execute("CREATE TABLE preserved(value)")
            if kind == "future":
                connection.execute("PRAGMA user_version=99")
    before = store.path.read_bytes()
    with pytest.raises((RuntimeError, sqlite3.DatabaseError)):
        await store.open()
    assert store.path.read_bytes() == before


async def test_fault_recovery_and_queue_bounds(core, tmp_path, monkeypatch):
    service, _, _, _ = core
    store = HistoryStore(service, tmp_path, True, 1)
    await store.open()
    for _ in range(256):
        store.queue.put_nowait(service.latest)
    store.schedule()
    assert store.dropped == 1 and store.queue.qsize() == 256
    while not store.queue.empty():
        store.queue.get_nowait()
    original = store.insert

    async def failure(_):
        raise sqlite3.OperationalError("database is full")

    monkeypatch.setattr(store, "insert", failure)
    store.queue.put_nowait(service.latest)
    task = asyncio.create_task(store.run())
    await asyncio.sleep(0.05)
    assert store.state == "degraded"
    assert service.snapshot().telemetry.state == "live"
    store.stopping = True
    await task
    monkeypatch.setattr(store, "insert", original)
    store.stopping = False
    await store.open()
    assert store.state == "ok"
    await store.close()


async def test_disabled_unavailable_and_cursor_bounds(core, tmp_path):
    store = HistoryStore(core[0], tmp_path, False, 10)
    assert store.health()["state"] == "disabled"
    with pytest.raises(AppError):
        await store.query()
    for value in ("garbage", "x" * 2049):
        with pytest.raises(AppError):
            CursorCodec().decode(value)
    codec = CursorCodec()
    assert codec.decode(codec.encode({"ok": 1})) == {"ok": 1}


def test_migration_only_on_online_backup_snapshot(tmp_path):
    original = tmp_path / "original.db"
    with closing(sqlite3.connect(original)) as source:
        source.execute("PRAGMA journal_mode=WAL")
        source.execute("CREATE TABLE preserved(value)")
        source.execute("INSERT INTO preserved VALUES(7)")
        source.commit()
        # Test the backup API while committed content can reside in the WAL.
        snapshot = tmp_path / "snapshot.db"
        with closing(sqlite3.connect(snapshot)) as target:
            source.backup(target)
            target.execute("ALTER TABLE preserved ADD COLUMN extra INTEGER")
            assert target.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert source.execute("SELECT * FROM preserved").fetchall() == [(7,)]


async def test_read_only_storage_preserves_live_service(core, tmp_path):
    store = HistoryStore(core[0], tmp_path, True, 10)
    await store.open()
    await store.close()
    store.path.chmod(0o400)
    try:
        with pytest.raises(sqlite3.OperationalError):
            await store.open()
        assert core[0].snapshot().telemetry.state == "live"
    finally:
        store.path.chmod(0o600)
        await store.close()


@pytest.mark.parametrize("seconds", [10, 60, 3600])
async def test_power_aggregates_include_zeros_skip_gaps_and_preserve_fractional_means(
    core, tmp_path, seconds
):
    service = core[0]
    store = HistoryStore(service, tmp_path, True, 10)
    await store.open()
    try:
        start = (int(service.latest.epoch) // seconds - 4) * seconds
        for offset, input_w, output_w in [(0, 0, 0), (1, 1, 3), (2 * seconds, 60, 180)]:
            sample = service.latest.sample.model_copy(
                update={"input_power_w": input_w, "output_power_w": output_w}
            )
            await store.insert(replace(service.latest, epoch=start + offset, sample=sample))
        # A reading exactly at until belongs to the next request, not this one.
        await store.insert(replace(service.latest, epoch=start + 3 * seconds))
        page = await store.aggregates(timestamp(start), timestamp(start + 3 * seconds), seconds, 3)
        assert page.bucket_seconds == seconds
        assert page.since_ms == start * 1000 and page.until_ms == (start + 3 * seconds) * 1000
        assert [item.bucket_start_ms for item in page.items] == [
            start * 1000,
            (start + 2 * seconds) * 1000,
        ]
        assert page.items[0].sample_count == 2
        assert page.items[0].input_power_w == 0.5 and page.items[0].output_power_w == 1.5
        assert page.items[1].sample_count == 1 and page.items[1].input_power_w == 60
        empty = await store.aggregates(
            timestamp(start + seconds), timestamp(start + 2 * seconds), seconds, 1
        )
        assert empty.items == []
        zero = await store.aggregates(
            timestamp(start + 3 * seconds), timestamp(start + 4 * seconds), seconds, 1
        )
        assert len(zero.items) == 1 and zero.items[0].input_power_w == 0
        assert zero.items[0].sample_count == 1
        async with store.db.execute(
            "EXPLAIN QUERY PLAN SELECT * FROM telemetry WHERE device_id=? "
            "AND received_at_ms>=? AND received_at_ms<?",
            (store.device_id, start * 1000, (start + seconds) * 1000),
        ) as query:
            assert "telemetry_device_time_id" in str(await query.fetchall())
        async with store.db.execute("PRAGMA integrity_check") as query:
            assert (await query.fetchone())[0] == "ok"
    finally:
        await store.close()


async def test_aggregate_validation_bounds_offsets_and_device_isolation(core, tmp_path):
    service = core[0]
    store = HistoryStore(service, tmp_path, True, 10)
    await store.open()
    try:
        await store.insert(replace(service.latest, epoch=-0.001))
        result = await store.aggregates("1969-12-31T23:59:50Z", "1970-01-01T00:00:00Z", 10, 1)
        assert result.items[0].bucket_start_ms == -10_000
        assert result.items[0].sample_count == 1
        equivalent = await store.aggregates(
            "1970-01-01T00:59:50+01:00", "1970-01-01T01:00:00+01:00", 10, 1
        )
        assert equivalent == result
        async with store.db.execute(
            "INSERT INTO devices(address,name,model,created_at_ms) "
            "VALUES ('other','Other','S300',0)"
        ) as query:
            other = query.lastrowid
        original = store.device_id
        store.device_id = other
        await store.insert(replace(service.latest, epoch=-0.002))
        store.device_id = original
        assert (
            await store.aggregates("1969-12-31T23:59:50Z", "1970-01-01T00:00:00Z", 10, 1)
        ).items[0].sample_count == 1
        for start, end, seconds, limit in [
            ("2026-10-05T12:00:00", "2026-10-05T12:01:00Z", 10, 10),
            ("2026-10-05T12:00:00Z", "2026-10-05T12:00:00Z", 10, 1),
            ("2026-10-05T12:00:00Z", "2026-10-05T13:00:00Z", 10, 256),
            ("2026-10-05T12:00:00Z", "2026-10-05T12:00:10Z", 11, 1),
            ("2026-10-05T12:00:00Z", "2026-10-05T12:00:10Z", 10, 0),
        ]:
            with pytest.raises(AppError) as caught:
                await store.aggregates(start, end, seconds, limit)
            assert caught.value.status == 422
        store.queries = 4
        with pytest.raises(AppError) as caught:
            await store.aggregates("1969-12-31T23:59:50Z", "1970-01-01T00:00:00Z")
        assert caught.value.status == 429
        store.queries = 0
        store.state = "degraded"
        with pytest.raises(AppError) as caught:
            await store.aggregates("1969-12-31T23:59:50Z", "1970-01-01T00:00:00Z")
        assert caught.value.status == 503
    finally:
        await store.close()


async def test_hourly_aggregates_return_bars_instead_of_thousands_of_raw_rows(core, tmp_path):
    store = HistoryStore(core[0], tmp_path, True, 10)
    await store.open()
    try:
        start = int(core[0].clock.time()) // 3600 * 3600 - 43 * 3600
        sample = core[0].latest.sample
        rows = [
            (
                store.device_id,
                (start + index * 10) * 1000,
                sample.segment_id,
                71,
                100 if index % 2 else 0,
                300 if index % 2 else 0,
                100,
                int(sample.ac_enabled),
                int(sample.dc_enabled),
                int(sample.light_enabled),
                sample.status_flags,
            )
            for index in range(43 * 360)
        ]
        await store.db.executemany(
            "INSERT INTO telemetry(device_id,received_at_ms,segment_id,battery_percent,"
            "input_power_w,output_power_w,remaining_minutes,ac_enabled,dc_enabled,"
            "light_enabled,status_flags) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            rows,
        )
        await store.db.commit()
        result = await store.aggregates(timestamp(start), timestamp(start + 43 * 3600), 3600, 43)
        assert len(result.items) == 43
        assert all(item.input_power_w == 50 and item.output_power_w == 150 for item in result.items)
        assert all(item.sample_count == 360 for item in result.items)
    finally:
        await store.close()


async def test_settings_persist_without_history_and_preserve_future_keys(core, tmp_path):
    store = HistoryStore(core[0], tmp_path, False, 10)
    await store.open()
    try:
        assert store.state == "disabled"
        assert (await store.settings()).graph_interval_seconds == 10
        await store.db.execute("INSERT INTO settings VALUES('future_preference','true')")
        await store.db.commit()
        assert (
            await store.settings(SettingsUpdate(graph_interval_seconds=60))
        ).graph_interval_seconds == 60
        assert (await store.settings(SettingsUpdate())).graph_interval_seconds == 60
        async with store.db.execute(
            "SELECT value_json FROM settings WHERE key='future_preference'"
        ) as query:
            assert (await query.fetchone())[0] == "true"
        async with store.db.execute("PRAGMA integrity_check") as query:
            assert (await query.fetchone())[0] == "ok"
    finally:
        await store.close()
    reopened = HistoryStore(core[0], tmp_path, False, 10)
    await reopened.open()
    try:
        assert (await reopened.settings()).graph_interval_seconds == 60
        with pytest.raises(AppError, match="disabled"):
            await reopened.query()
    finally:
        await reopened.close()
    with pytest.raises(AppError) as unavailable:
        await reopened.settings(SettingsUpdate(graph_interval_seconds=3600))
    assert unavailable.value.code == "settings_unavailable"


async def test_settings_table_addition_preserves_a_consistent_existing_database_snapshot(
    core, tmp_path
):
    source_dir = tmp_path / "source"
    source_dir.mkdir()
    original = HistoryStore(core[0], source_dir, True, 10)
    await original.open()
    await original.insert(core[0].latest)
    await original.close()
    target_dir = tmp_path / "snapshot"
    target_dir.mkdir()
    # Model the existing schema, then test only its Online Backup snapshot.
    with closing(sqlite3.connect(source_dir / "mypowers.db")) as source:
        source.execute("DROP TABLE settings")
        source.commit()
        with closing(sqlite3.connect(target_dir / "mypowers.db")) as target:
            source.backup(target)
    store = HistoryStore(core[0], target_dir, True, 10)
    await store.open()
    try:
        assert (await store.settings()).graph_interval_seconds == 10
        await store.settings(SettingsUpdate(graph_interval_seconds=3600))
        assert len((await store.query(until=timestamp(core[0].clock.time() + 1))).items) == 1
        async with store.db.execute("PRAGMA integrity_check") as query:
            assert (await query.fetchone())[0] == "ok"
    finally:
        await store.close()
    with closing(sqlite3.connect(source_dir / "mypowers.db")) as source:
        assert (
            source.execute("SELECT name FROM sqlite_master WHERE name='settings'").fetchone()
            is None
        )
