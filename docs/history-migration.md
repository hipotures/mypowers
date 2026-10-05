# Change-only telemetry history

Schema version 2 stores `telemetry_states`, with one complete payload per change.
Consecutive identical payloads in a continuous segment extend the same span instead
of adding rows. Every field participates, including battery and remaining minutes.
Changes are recorded immediately; `history.interval_seconds` controls coverage
checkpoints for unchanged values (default 10 seconds). Checkpoints still write to
SQLite, but do not duplicate telemetry payloads.

`received_at_ms` starts a state, `end_at_ms` is its exclusive end, and
`last_observed_at_ms` is the last confirmed observation of that payload. A change
closes the preceding state at the change timestamp. The current state's coverage
ends at its last recorded observation plus one millisecond, not at the current
wall clock. That millisecond represents an isolated observation. A process crash
can lose up to one checkpoint interval of the unchanged tail. Disconnects, stale
telemetry, rejected frames, recording failures and clock jumps break continuity.

Power aggregates split these spans across UTC buckets in SQLite and calculate
`SUM(power * covered_duration) / SUM(covered_duration)`. Recorded zero contributes;
unavailable time does not. No permanent aggregate tables or additional dependency
are required. Raw history returns state changes, not reconstructed periodic samples.

## Explicit legacy conversion

The daemon accepts only version 2. It does not migrate or read the legacy table.
The conversion script creates a consistent snapshot with SQLite Online Backup,
converts the snapshot in a transaction, verifies every old observation's full
payload and timestamp against its assigned span, independently compares covered
duration and INPUT/OUTPUT integrals per device, and runs `integrity_check` and
`foreign_key_check`. It refuses existing output paths and never edits the source.

From the repository root, using the existing server environment:

```bash
.venv/bin/python scripts/migrate-history.py \
  .local/dev/data/mypowers.db /tmp/mypowers-history-v2/mypowers.db \
  --interval-seconds 10
```

Set `--interval-seconds` to the old **recording** interval, not the graph interval.
The old history cannot recover changes that occurred between recorded samples.
Values are held until the next recorded observation within a continuous segment.
A segment change, backwards clock, or gap of at least twice the recording interval
ends that inference at the last observation plus one millisecond. Equal timestamps
retain their distinct payloads; earlier changes at the same timestamp have zero
duration. Settings and device metadata remain intact.

The script prints success only after verification, with original and compressed
row counts. Failures exit nonzero and remove the incomplete output. The legacy
`telemetry` table remains in the successful snapshot for inspection.

## Deployment and later cleanup

For a deployment, stop the old daemon **before** taking the final snapshot so no
later measurements are left behind. Generate it in a new, persistent data directory,
then point `MYPOWERS_DATA_DIR` at that directory and start the new daemon. Keep the
old directory intact. A snapshot produced earlier while the daemon was running is
appropriate for testing, not a complete replacement for its later history.

After verifying the new API and history, the old table can be removed explicitly
from the new database. Stop the daemon during this cleanup:

```bash
sqlite3 /path/to/new-data/mypowers.db \
  'BEGIN IMMEDIATE; DROP TABLE telemetry; COMMIT; PRAGMA integrity_check; VACUUM;'
```

`VACUUM` reclaims pages occupied by the retained legacy table. New databases do
not contain that table. Migration experiments must always use a fresh snapshot;
never run them against the original database.
