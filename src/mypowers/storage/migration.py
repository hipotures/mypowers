"""Explicit legacy-history conversion, performed only on an Online Backup snapshot."""

import argparse
import math
import sqlite3
import sys
from contextlib import closing
from pathlib import Path

from mypowers.storage.schema import INSERT_STATE, STATE_FIELDS, STATES_SCHEMA, VERSION


def migrate(source: Path, output: Path, interval_seconds: float = 10) -> tuple[int, int]:
    """Return old/new row counts after a verified atomic conversion of a new snapshot.

    The legacy table remains intact. Its inferred coverage stops at each segment's
    last observed millisecond; missing recording intervals are not interpolated.
    """
    if not math.isfinite(interval_seconds) or not 0 < interval_seconds <= 3600:
        raise ValueError("Recording interval must be greater than zero and at most 3600 seconds.")
    source, output = source.resolve(), output.resolve()
    if source == output:
        raise ValueError("Source and output must differ; conversion never edits the source.")
    with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as original:
        if original.execute("PRAGMA user_version").fetchone()[0] != 1:
            raise ValueError("Source must use legacy schema version 1.")
        output.parent.mkdir(parents=True, exist_ok=True)
        # Exclusive creation also rejects hard links to the source and prior outputs.
        with output.open("xb"):
            pass
        try:
            with closing(sqlite3.connect(output)) as database:
                original.backup(database)
                database.execute("PRAGMA journal_mode=DELETE")
                database.execute("PRAGMA synchronous=FULL")
                database.execute("PRAGMA foreign_keys=ON")
                verify_integrity(database)
                counts = convert(database, round(interval_seconds * 2000))
                verify_integrity(database)
                return counts
        except BaseException:
            output.unlink(missing_ok=True)
            raise


def verify_integrity(database: sqlite3.Connection) -> None:
    if database.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
        raise ValueError("SQLite integrity check failed.")
    if database.execute("PRAGMA foreign_key_check").fetchone() is not None:
        raise ValueError("SQLite foreign key check failed.")


def convert(database: sqlite3.Connection, max_gap_ms: int) -> tuple[int, int]:
    database.executescript("BEGIN IMMEDIATE;\n" + STATES_SCHEMA)
    try:
        database.execute(
            "CREATE TEMP TABLE migration_members("
            "old_id INTEGER PRIMARY KEY, state_id INTEGER NOT NULL)"
        )
        previous = None
        old_count = 0
        columns = ",".join(STATE_FIELDS)
        for row in database.execute(
            "SELECT id,device_id,received_at_ms,segment_id,"
            + columns
            + " FROM telemetry ORDER BY device_id,id"
        ):
            old_id, device, received, segment = row[:4]
            values = tuple(row[4:])
            continuous = (
                previous is not None
                and device == previous[0]
                and segment == previous[2]
                and 0 <= received - previous[1] < max_gap_ms
            )
            stored_segment = (
                previous[3] if previous is not None and continuous else f"legacy:{old_id}:{segment}"
            )
            if previous is not None and continuous and values == previous[4]:
                state_id = previous[5]
                database.execute(
                    "UPDATE telemetry_states SET end_at_ms=?,last_observed_at_ms=? WHERE id=?",
                    (received + 1, received, state_id),
                )
            else:
                if previous is not None and continuous:
                    database.execute(
                        "UPDATE telemetry_states SET end_at_ms=? WHERE id=?",
                        (received, previous[5]),
                    )
                state_id = database.execute(
                    INSERT_STATE,
                    (device, received, received + 1, received, stored_segment, *values),
                ).lastrowid
            database.execute("INSERT INTO migration_members VALUES (?,?)", (old_id, state_id))
            previous = (device, received, segment, stored_segment, values, state_id)
            old_count += 1
        # Every old observation must retain its complete payload and timestamp in
        # its assigned state span, including equal timestamps and clock reversals.
        mismatches = " OR ".join(f"t.{field} != s.{field}" for field in STATE_FIELDS)
        invalid = database.execute(
            "SELECT count(*) FROM telemetry t LEFT JOIN migration_members m ON m.old_id=t.id "
            "LEFT JOIN telemetry_states s ON s.id=m.state_id WHERE s.id IS NULL "
            "OR t.device_id != s.device_id OR t.received_at_ms < s.received_at_ms "
            "OR t.received_at_ms > s.last_observed_at_ms OR (" + mismatches + ")"
        ).fetchone()[0]
        if invalid or database.execute("SELECT count(*) FROM telemetry").fetchone()[0] != old_count:
            raise ValueError("Legacy observation verification failed.")
        # Independently verify duration and power integrals directly from the old
        # adjacent observations. This also checks coverage for all-zero histories.
        expected = database.execute(
            "WITH following AS (SELECT *, "
            "LEAD(received_at_ms) OVER w AS next_time, LEAD(segment_id) OVER w AS next_segment "
            "FROM telemetry WINDOW w AS (PARTITION BY device_id ORDER BY id)), "
            "durations AS (SELECT *, CASE WHEN next_segment=segment_id "
            "AND next_time-received_at_ms>=0 AND next_time-received_at_ms<? "
            "THEN next_time-received_at_ms ELSE 1 END AS duration FROM following) "
            "SELECT device_id,SUM(duration),SUM(input_power_w*duration),"
            "SUM(output_power_w*duration) "
            "FROM durations GROUP BY device_id ORDER BY device_id",
            (max_gap_ms,),
        ).fetchall()
        actual = database.execute(
            "SELECT device_id,SUM(end_at_ms-received_at_ms),"
            "SUM(input_power_w*(end_at_ms-received_at_ms)),"
            "SUM(output_power_w*(end_at_ms-received_at_ms)) "
            "FROM telemetry_states GROUP BY device_id ORDER BY device_id"
        ).fetchall()
        if actual != expected:
            raise ValueError("Duration or power integral verification failed.")
        new_count = database.execute("SELECT count(*) FROM telemetry_states").fetchone()[0]
        verify_integrity(database)
        database.execute(f"PRAGMA user_version={VERSION}")
        database.execute("DROP TABLE migration_members")
        database.commit()
        return old_count, new_count
    except BaseException:
        database.rollback()
        raise


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Back up and convert legacy history; retain the source and legacy table."
    )
    parser.add_argument("source", type=Path, help="Existing schema-v1 SQLite database")
    parser.add_argument("output", type=Path, help="New snapshot path; must not exist")
    parser.add_argument(
        "--interval-seconds",
        type=float,
        default=10,
        help="Legacy recording interval (default: 10); not the graph interval",
    )
    args = parser.parse_args()
    try:
        old, new = migrate(args.source, args.output, args.interval_seconds)
    except (ValueError, OSError, sqlite3.Error) as error:
        print(f"Migration failed: {error}", file=sys.stderr)
        raise SystemExit(1) from None
    print(
        f"Verified {args.output}: {old} observations -> {new} state spans. Legacy table retained."
    )


if __name__ == "__main__":
    main()
