# API v1

The daemon uses loopback HTTP in production behind Caddy HTTPS/WSS. All application endpoints
require `Authorization: Bearer ...` when authentication is enabled. `GET /health/live` returns
only `{"status":"ok"}`. It measures HTTP liveness independently of station availability. Status
returns 200 with independent connection, freshness, qualification, history and logging health.
API responses have `Cache-Control: no-store`. Production disables interactive docs/OpenAPI serving;
the checked-in [OpenAPI artifact](openapi.json) describes typed requests and responses.

| Method/path | Purpose |
|---|---|
| GET `/api/v1/status` | Complete immutable transport snapshot |
| GET `/api/v1/capabilities` | Only qualified reads and AC/DC/common-lamp outputs |
| GET `/api/v1/settings` | Persisted application settings and defaults |
| PUT `/api/v1/settings` | Validate and persist supplied settings fields; omitted fields stay unchanged |
| GET `/api/v1/history` | UTC `[since,until)` state change page, limit/cursor |
| GET `/api/v1/history/aggregates` | UTC duration-weighted power averages per 10s / 30s / 60s / 1h bucket |
| PUT `/api/v1/outputs/{ac,dc,light}` | One explicit boolean intention with idempotency UUID |
| GET `/api/v1/commands/{uuid}` | Retained asynchronous result |
| PUT `/api/v1/connection` | `{"desired":"paused"}` or `{"desired":"running"}` |
| POST `/api/v1/connection/retry` | Empty JSON object; coalesced wakeup |
| GET `/api/v1/logs` | Tail or UTC range, min_level/limit/cursor/direction |
| PUT `/api/v1/runtime/log-level` | DEBUG/INFO/WARNING/ERROR, optional duration_seconds ≤86400 |
| DELETE `/api/v1/runtime/log-level` | Remove override and restore startup baseline |
| WS `/api/v1/events` | Initial snapshot, state/command/heartbeat observations |
| WS `/api/v1/logs/stream` | Snapshot, retained log records then live records, gap/heartbeat |

A mutation needs JSON Content-Type. Unknown fields, string booleans and invalid enums fail.
Host is restricted to loopback or configured public hostname; untrusted browser Origin fails.
HTTP codes: 401 auth, 403 policy, 409 busy/conflict/profile, 422 validation, 413 body limit,
429 admission limits, 503 unavailable required subsystem. Errors never echo raw request input:

```json
{"schema_version":1,"error":{"code":"state_conflict","message":"Station output state changed; refresh before retrying.","retryable":false,"request_id":"uuid"}}
```

Control example (`Idempotency-Key` is a UUID):

```json
{"enabled":true,"server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10","expected_outputs_revision":3}
```

202 admits a daemon-owned command; the client polls its resource. Pending states are accepted,
waiting_for_status and sent. Terminal states are confirmed, no_change, rejected, failed and
unconfirmed. Two consecutive post-send complete flags from the active session are required for
confirmed. Dynamic numeric fields may vary. No-change requires a new live sample and sends nothing.
A potentially transmitted but unconfirmed operation forces session resynchronization and is never
replayed. Client loss cannot cancel an admitted operation or trigger automatic restoration.

The output revision advances on complete flags changes or revoked authorization, not numeric-only
updates. A server UUID protects against replay after restart. Idempotent retries return the existing
operation before busy checks; different normalized bodies with the same key conflict. Keys/results
are bounded in RAM and not persisted across crashes. Frontends never automatically resubmit PUTs.

Snapshot fields are documented by `Status`, `Connection`, `Telemetry`, `Sample`, `Controls` and
`Command` in OpenAPI. `telemetry.sample` is null before observation; stale last-known readings remain
visible. All receive/log times use aware RFC3339 UTC milliseconds; history stores receive epoch
milliseconds. Sample age/deadlines use monotonic time. `state_version`, receive sequence, stream
sequence and output revision are distinct counters. Segments break on acquisition/recording gaps
and wall-clock jumps. Zero is a real value; remaining_minutes=0 displays `0h 00m`.

History default range is last hour ending at server time; limit defaults 1,000, maximum 10,000.
Pages order by `(received_at_ms,id)` and keep a signed high-water ID to exclude subsequent inserts,
including backward clock insertions. Pass the cursor with unchanged or omitted range filters.
Disabled/degraded history is a 503, distinct from a successful empty page.
Rows contain full state payloads and `received_at_ms`, exclusive `end_at_ms`,
and `last_observed_at_ms`. Range filters select changes starting within the range;
aggregate queries also include overlapping states that started before it.
Coverage endpoints can advance while a state remains unchanged; high-water
pagination excludes newly inserted changes, not updates to coverage endpoints.

`GET /api/v1/history/aggregates` requires aware `since` and `until` timestamps.
`bucket_seconds` is 10 (default), 30, 60 or 3600; `limit` is the maximum number of
potential buckets in the requested range, 1–256 (default 256). Oversized ranges
return 422, even when little or no data exists; aggregate results are not paged
or silently truncated. The current device's indexed state spans are aggregated
in SQLite, without additional database files or permanent aggregate tables.

Buckets align to UTC epoch multiples of `bucket_seconds`. Filtering is exactly
`[since,until)`, so an unaligned range can include partial first/last buckets.
The response echoes `bucket_seconds`, `since_ms`, `until_ms` and `source=database`.
Ordered `items` contain `bucket_start_ms`, fractional `input_power_w` and
`output_power_w` averages, and `sample_count` (the number of contributing state
spans). Averages are `SUM(power * covered_duration) / SUM(covered_duration)`;
recorded zeros contribute. Missing buckets are omitted and must remain gaps in
a graph. Unavailable periods do not contribute and are never filled from cached
telemetry. The same availability,
authentication, four-query admission limit and six-second deadline apply to raw
and aggregate history reads.

```text
GET /api/v1/history/aggregates?since=2026-10-05T00:00:00Z&until=2026-10-05T01:00:00Z&bucket_seconds=60&limit=60
```

Log default page is 100, maximum 1,000; CLI defaults tail 10. Tail and range/cursor are exclusive.
Queries read only active/retained application files off-loop with a 3-second/64-MiB scan budget.
Signed cursors reference file identities/offsets; removed sources return 410 log_cursor_expired.
Malformed/final partial lines are skipped with a count. Ring fallback explicitly sets source=ring,
gap=true. Logs include raw frame_hex only at DEBUG, never in SQLite or normal status/command DTOs.

Range queries accept `direction=forward` (oldest first, default) or `backward`
(newest page first); the records within every page are chronological. Responses
include `previous_cursor`, `next_cursor`, `has_more_before`, and `has_more_after`.
Fetch an older page with its `previous_cursor` and `direction=backward`, or a
newer page with its `next_cursor` and `direction=forward`. Keep the same time and
minimum-level filters. Cursors identify byte boundaries or ring sequences and
survive file renaming during retained rotation. Expired/truncated sources return
410; a client must explicitly refresh rather than silently omit history.

The TUI translates the selected local calendar day into UTC `[since,until)`
boundaries and loads bounded pages. DST days can be 23 or 25 hours. Archive
scrolling does not insert live records into the displayed window; today/live
is an explicit follow state. Log-level changes are INFO audit events, including
when normal INFO logging has been disabled.

## Stream schema

All streams send a `snapshot` first. Each message has schema_version=1, type,
server_instance_id, monotonically increasing connection-local stream_sequence, server_time and
optional data. State data is a full status; command data is a Command; log data is a JSONL record.
Heartbeats arrive at least every five seconds. Clients consider server connectivity lost after
15 seconds without a valid message. Reachable-server station loss is displayed separately.

```json
{"schema_version":1,"type":"heartbeat","server_instance_id":"uuid","stream_sequence":3,"server_time":"2026-10-04T12:00:00.000Z","data":null}
```

Native WS clients authenticate in handshake headers. Browser clients may authenticate as their
first message `{"type":"authenticate","token":"..."}` within five seconds; no application data is
sent beforehand. Tokens in query parameters are refused. Browser Origin is validated before
upgrade. WS messages are observational: no socket command can control the station. Log streams
accept cursor/min_level query filters; tokens never belong there. Expired cursor closes explicitly.
After reconnect, discard old stream assumptions, accept the new full snapshot and query retained
command IDs. Slow queues close 1013; command results remain queryable. No durable state replay.


## Application settings

`GET /api/v1/settings` returns the complete public preferences document:

```json
{"schema_version":1,"graph_interval_seconds":10,"graph_visualization":"sparkline","graph_base_scale_w":100,"timezone":"system","logs_page_size":100}
```

`PUT /api/v1/settings` writes only supplied fields and returns the complete saved
settings. Supported values:

| Field | Values |
|---|---|
| `graph_interval_seconds` | Integer 10, 30, 60, 3600 seconds per bar |
| `graph_visualization` | `sparkline`, `chart` |
| `graph_base_scale_w` | Integer 100 or 300 W; automatic doubling above the base |
| `timezone` | `system` or a valid IANA timezone |
| `logs_page_size` | Integer 50, 100, 250, 500, 1000 |

Unknown fields, null, invalid types and unsupported values return 422 without
changing any preference. An empty object leaves saved values untouched. Settings
use the same authentication and request policies as other endpoints. Unavailable
SQLite storage returns 503.

Values survive daemon restarts in the existing `settings(key,value_json)` table,
independently of telemetry recording. No table alteration is needed for new
preference fields. YAML and dotenv are not rewritten. The TUI Settings form offers
visible lists and a Save changes button; a confirmed save applies all preferences
immediately and future TUI sessions load them at startup. Unsaved edits remain a
draft. Explicit TUI timezone flags override the saved timezone at startup.
Dashboard graph shortcuts change only that session. Debug connection controls
and the runtime log override are actions, separate from persisted preferences.
Connector credentials must not be added to this client-visible document.
