import sqlite3
import subprocess
import sys
from contextlib import closing
from pathlib import Path

import pytest

from mypowers.contracts import timestamp
from mypowers.storage import HistoryStore
from mypowers.storage.migration import migrate
from mypowers.storage.schema import AGGREGATES, STATE_FIELDS

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "migrate-history.py"


def legacy(database):
    database.executescript("""
        CREATE TABLE devices(id INTEGER PRIMARY KEY, address TEXT UNIQUE, name TEXT,
                             model TEXT, created_at_ms INTEGER);
        INSERT INTO devices VALUES(1,'first','Station','S300',0),(2,'second','Other','S300',0);
        CREATE TABLE settings(key TEXT PRIMARY KEY,value_json TEXT NOT NULL) STRICT;
        INSERT INTO settings VALUES('graph_interval_seconds','30');
        CREATE TABLE telemetry(
          id INTEGER PRIMARY KEY, device_id INTEGER NOT NULL REFERENCES devices(id),
          received_at_ms INTEGER NOT NULL, segment_id TEXT NOT NULL,
          battery_percent INTEGER NOT NULL, input_power_w INTEGER NOT NULL,
          output_power_w INTEGER NOT NULL, remaining_minutes INTEGER NOT NULL,
          ac_enabled INTEGER NOT NULL, dc_enabled INTEGER NOT NULL,
          light_enabled INTEGER NOT NULL, status_flags INTEGER NOT NULL
        ) STRICT;
        CREATE INDEX telemetry_device_time_id ON telemetry(device_id,received_at_ms,id);
        PRAGMA user_version=1;
    """)


def add(database, received, *, device=1, segment="a", **changes):
    state = dict(zip(STATE_FIELDS, (71, 0, 0, 100, 0, 0, 0, 12), strict=True))
    state.update(changes)
    database.execute(
        "INSERT INTO telemetry(device_id,received_at_ms,segment_id,"
        + ",".join(STATE_FIELDS)
        + ") VALUES ("
        + ",".join(["?"] * 11)
        + ")",
        (device, received, segment, *state.values()),
    )


def test_online_backup_conversion_preserves_all_payloads_gaps_devices_and_settings(tmp_path):
    source, output = tmp_path / "source.db", tmp_path / "snapshot.db"
    with closing(sqlite3.connect(source)) as original:
        original.execute("PRAGMA journal_mode=WAL")
        legacy(original)
        for time in (0, 10000, 20000):
            add(original, time)
        add(original, 30000, input_power_w=100)
        add(original, 40000, input_power_w=100)
        add(original, 50000, input_power_w=100, remaining_minutes=99)
        # Explicit segment break, missing recording interval, backwards clock,
        # and a second device all forbid merging even when payloads are identical.
        add(original, 51000, segment="b", input_power_w=100, remaining_minutes=99)
        add(original, 71000, segment="b", input_power_w=100, remaining_minutes=99)
        add(original, 69000, segment="b", input_power_w=100, remaining_minutes=99)
        add(original, 69000, segment="b", input_power_w=100, remaining_minutes=99)
        add(original, 0, device=2)
        original.commit()
        before = original.execute("SELECT * FROM telemetry ORDER BY id").fetchall()
        assert Path(str(source) + "-wal").exists()
        assert migrate(source, output) == (11, 7)
        assert original.execute("SELECT * FROM telemetry ORDER BY id").fetchall() == before
        assert original.execute("PRAGMA user_version").fetchone()[0] == 1
        assert not original.execute(
            "SELECT name FROM sqlite_master WHERE name='telemetry_states'"
        ).fetchall()
        with closing(sqlite3.connect(output)) as converted:
            assert converted.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
            assert converted.execute("PRAGMA foreign_key_check").fetchall() == []
            assert converted.execute("PRAGMA user_version").fetchone()[0] == 2
            assert converted.execute("SELECT * FROM telemetry ORDER BY id").fetchall() == before
            assert converted.execute("SELECT * FROM settings").fetchall() == [
                ("graph_interval_seconds", "30")
            ]
            assert converted.execute(
                "SELECT received_at_ms,end_at_ms,last_observed_at_ms FROM telemetry_states "
                "WHERE device_id=1 ORDER BY id"
            ).fetchall() == [
                (0, 30000, 20000),
                (30000, 50000, 40000),
                (50000, 50001, 50000),
                (51000, 51001, 51000),
                (71000, 71001, 71000),
                (69000, 69001, 69000),
            ]


def test_migrated_duration_means_cover_buckets_and_never_bridge_outages(tmp_path):
    source, output = tmp_path / "source.db", tmp_path / "snapshot.db"
    with closing(sqlite3.connect(source)) as original:
        legacy(original)
        # 100 W for one minute, then 0 W for nine minutes => 10 W, not 50 W.
        for time in range(0, 600000, 10000):
            add(original, time, input_power_w=100 if time < 60000 else 0)
        add(original, 600000, input_power_w=0)
        add(original, 720000, segment="reconnected", input_power_w=20)
        add(original, 730000, segment="reconnected", input_power_w=20)
        original.commit()
    assert migrate(source, output) == (63, 3)
    with closing(sqlite3.connect(output)) as database:
        mean = database.execute(
            AGGREGATES,
            (0, 3600000, 3600000, 600000, 1, 0, 600000, 3600000, 600000, 0, 3600000),
        ).fetchall()
        assert mean == [(0, 10.0, 0.0, 2)]
        missing = database.execute(
            AGGREGATES,
            (610000, 10000, 10000, 720000, 1, 610000, 720000, 10000, 720000, 610000, 10000),
        ).fetchall()
        assert missing == []
        # Verify the whole migration's time-weighted power integral independently,
        # using legacy adjacent points and explicit boundaries rather than spans.
        points = database.execute(
            "SELECT received_at_ms,input_power_w,segment_id FROM telemetry ORDER BY id"
        ).fetchall()
        expected = sum(
            p[1] * (q[0] - p[0])
            for p, q in zip(points, points[1:], strict=False)
            if p[2] == q[2] and 0 <= q[0] - p[0] < 20000
        )
        expected += points[60][1] + points[-1][1]
        actual = database.execute(
            "SELECT sum(input_power_w * (end_at_ms-received_at_ms)) FROM telemetry_states"
        ).fetchone()[0]
        assert actual == expected


def test_equal_timestamps_keep_last_state_without_phantom_duration(tmp_path):
    source, output = tmp_path / "source.db", tmp_path / "snapshot.db"
    with closing(sqlite3.connect(source)) as original:
        legacy(original)
        add(original, 0, input_power_w=100)
        add(original, 0, input_power_w=20)
        add(original, 10000, input_power_w=20)
        original.commit()
    assert migrate(source, output) == (3, 2)
    with closing(sqlite3.connect(output)) as database:
        assert database.execute(
            "SELECT received_at_ms,end_at_ms FROM telemetry_states ORDER BY id"
        ).fetchall() == [(0, 0), (0, 10001)]
        assert database.execute(
            AGGREGATES,
            (0, 10000, 10000, 10000, 1, 0, 10000, 10000, 10000, 0, 10000),
        ).fetchall() == [(0, 20.0, 0.0, 1)]


def test_failures_leave_original_and_existing_output_untouched(tmp_path):
    source, output = tmp_path / "source.db", tmp_path / "snapshot.db"
    with closing(sqlite3.connect(source)) as original:
        legacy(original)
        add(original, 0, battery_percent=101)
        original.commit()
    before = source.read_bytes()
    with pytest.raises(sqlite3.IntegrityError):
        migrate(source, output)
    assert not output.exists() and source.read_bytes() == before
    with pytest.raises(ValueError, match="differ"):
        migrate(source, source)
    output.write_bytes(b"preserved")
    with pytest.raises(FileExistsError):
        migrate(source, output)
    assert output.read_bytes() == b"preserved"
    for interval in (0, -1, float("nan"), float("inf"), 3601):
        with pytest.raises(ValueError, match="interval"):
            migrate(source, tmp_path / "bad.db", interval)
    assert not (tmp_path / "bad.db").exists()


def test_empty_database_cli_success_and_failure_exit_codes(tmp_path):
    source, output = tmp_path / "source.db", tmp_path / "snapshot.db"
    with closing(sqlite3.connect(source)) as original:
        legacy(original)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), str(source), str(output)], capture_output=True, text=True
    )
    assert result.returncode == 0 and "0 observations -> 0 state spans" in result.stdout
    repeated = subprocess.run(
        [sys.executable, str(SCRIPT), str(source), str(output)], capture_output=True, text=True
    )
    assert repeated.returncode == 1 and not repeated.stdout
    assert "Migration failed" in repeated.stderr
    with pytest.raises(ValueError, match="version 1"):
        migrate(output, tmp_path / "future.db")
    assert not (tmp_path / "future.db").exists()


async def test_current_store_reads_migrated_history_and_starts_a_new_segment(core, tmp_path):
    service = core[0]
    source, output = tmp_path / "source.db", tmp_path / "new" / "mypowers.db"
    start = int(service.latest.epoch * 1000) - 120000
    with closing(sqlite3.connect(source)) as original:
        legacy(original)
        original.execute("UPDATE devices SET address=? WHERE id=1", (service.address,))
        for offset in range(0, 120000, 10000):
            add(original, start + offset, input_power_w=10, output_power_w=20)
        original.commit()
    assert migrate(source, output) == (12, 1)
    store = HistoryStore(service, output.parent, True, 10)
    await store.open()
    try:
        assert (await store.settings()).graph_interval_seconds == 30
        before = await store.query(timestamp(start / 1000), timestamp(service.clock.time()))
        assert len(before.items) == 1 and before.items[0]["input_power_w"] == 10
        aggregates = await store.aggregates(
            timestamp(start / 1000),
            timestamp(service.clock.time()),
            10,
            13,
        )
        assert len(aggregates.items) >= 10
        assert all(
            item.input_power_w == 10 and item.output_power_w == 20 for item in aggregates.items
        )
        await store.insert(service.latest)
        after = await store.query(timestamp(start / 1000), timestamp(service.clock.time() + 1))
        assert len(after.items) == 2
        assert after.items[0]["segment_id"] != after.items[1]["segment_id"]
        async with store.db.execute("PRAGMA integrity_check") as query:
            assert (await query.fetchone())[0] == "ok"
    finally:
        await store.close()
