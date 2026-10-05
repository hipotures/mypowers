"""Current telemetry schema; legacy schemas are handled only by the operator script."""

VERSION = 2
STATE_FIELDS = (
    "battery_percent",
    "input_power_w",
    "output_power_w",
    "remaining_minutes",
    "ac_enabled",
    "dc_enabled",
    "light_enabled",
    "status_flags",
)

STATES_SCHEMA = """
CREATE TABLE telemetry_states (
 id INTEGER PRIMARY KEY, device_id INTEGER NOT NULL REFERENCES devices(id),
 received_at_ms INTEGER NOT NULL, end_at_ms INTEGER NOT NULL,
 last_observed_at_ms INTEGER NOT NULL, segment_id TEXT NOT NULL,
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
 CHECK (light_enabled = ((status_flags & 16) != 0)),
 CHECK (received_at_ms <= last_observed_at_ms AND last_observed_at_ms <= end_at_ms)
) STRICT;
CREATE INDEX states_device_time_id ON telemetry_states(device_id, received_at_ms, id);
CREATE INDEX states_device_end ON telemetry_states(device_id, end_at_ms);
CREATE INDEX states_device_id ON telemetry_states(device_id, id);
"""

INSERT_STATE = (
    "INSERT INTO telemetry_states(device_id,received_at_ms,end_at_ms,last_observed_at_ms,"
    "segment_id," + ",".join(STATE_FIELDS) + ") VALUES (" + ",".join(["?"] * 13) + ")"
)

SCHEMA = (
    """
BEGIN IMMEDIATE;
CREATE TABLE devices (
 id INTEGER PRIMARY KEY, address TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 model TEXT NOT NULL, created_at_ms INTEGER NOT NULL
) STRICT;
"""
    + STATES_SCHEMA
    + """
PRAGMA user_version=2;
COMMIT;
"""
)

# Only known intervals contribute. A lone or final observation covers its own
# millisecond, never the time after it; disconnected tails cannot extend to now.
AGGREGATES = """
WITH RECURSIVE buckets(bucket_start_ms) AS (
 SELECT ? UNION ALL SELECT bucket_start_ms + ? FROM buckets
 WHERE bucket_start_ms + ? < ?
), spans AS (
 SELECT * FROM telemetry_states
 WHERE device_id=? AND end_at_ms>? AND received_at_ms<?
), overlaps AS (
 SELECT b.bucket_start_ms, s.input_power_w, s.output_power_w,
 min(s.end_at_ms,b.bucket_start_ms+?,?) - max(s.received_at_ms,b.bucket_start_ms,?) AS duration
 FROM buckets b JOIN spans s
 ON s.received_at_ms < b.bucket_start_ms+? AND s.end_at_ms > b.bucket_start_ms
)
SELECT bucket_start_ms,
 SUM(input_power_w * 1.0 * duration) / SUM(duration),
 SUM(output_power_w * 1.0 * duration) / SUM(duration), COUNT(*)
FROM overlaps WHERE duration>0 GROUP BY bucket_start_ms ORDER BY bucket_start_ms
"""
