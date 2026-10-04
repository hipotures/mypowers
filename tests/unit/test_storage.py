import asyncio
import sqlite3
from contextlib import closing
from dataclasses import replace
from uuid import uuid4

import pytest
from conftest import frame

from mypowers.contracts import AppError, timestamp
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
