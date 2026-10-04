# Operations and troubleshooting

The daemon is a monitor and explicit controller. It never enforces a desired output state,
re-enables a spontaneously disabled output, probes unsupported commands, powers/unblocks an
adapter, pairs/trusts a station, restarts BlueZ or changes outputs at startup/shutdown. Pause releases
only its BLE session so another app can connect; outputs remain unchanged. Retry wakes/coalesces
acquisition; a healthy connection is retained. Pause is rejected during an output operation.

## Observable health

`mypowers status` succeeds even if the station is missing; `--require-live` returns exit 5 when
telemetry is not live. Server reachability, link state, sample freshness, qualified profile and
history/logging health are independent. A connected link with ≥3s sample age is stale; it cannot
write. At 10s silence the supervisor cleans up serially and reconnects. First-sample wait is 5s.
Invalid packets revoke eligibility without replacing valid measurements with invented zeros.
Older-session callbacks are discarded. Failed cleanup retains the client and blocks a new one.

| Reason | Operator checks |
|---|---|
| system_bus_unavailable | System D-Bus socket/service/access |
| bluez_unavailable | BlueZ service availability on the accessible bus |
| permission_denied | Operation/account D-Bus/BlueZ permissions |
| adapter_missing | Intended MAC, USB/driver and guest/LXC exposure |
| adapter_off / adapter_blocked | Observed power/rfkill condition; enable/unblock externally if intended |
| adapter_not_ready | Properties cannot distinguish power/block/driver state; inspect locally |
| station_not_found | Station power, Bluetooth, range, other apps; no unique contention diagnosis |
| no_telemetry / telemetry_silence | Correct subscription exists but valid data is absent; inspect logs |
| cleanup_failed | Previous session could not be released; wait/retry cleanup or inspect daemon |
| unqualified_profile / unqualified_identity | No write qualification for current complete state/unit |

Do not diagnose phone contention or overheating from generic absence or AC-off flags. Name `V2.0`
is not a measured firmware revision. USB-C behavior is not inferred from the common DC flag.
Input/output are aggregate watts; v1 computes no battery charging watts, Wh or guaranteed runtime.

## Commands and diagnostics

Use explicit `ac/dc/light on/off`, never blind hardware toggles. A frontend captures server UUID and
output revision, sends one intention and polls a command ID. Admission locks fresh read, encoding,
single write and two-sample confirmation. A successful BLE send alone is not success. An unknown
outcome may have changed outputs: query status/command and make a later deliberate fresh intention;
do not replay the old PUT. A crash loses in-memory idempotency retention; old server UUIDs fail.

CLI exit codes: 0 success/status reachable, 1 query/application error, 2 usage/configuration,
3 server/TLS/response transport failure, 4 auth/policy, 5 station/control precondition/conflict/busy,
6 unknown/unconfirmed/wait-expired control, 130 interruption. JSON output contains one complete
response/error object; `logs --follow --json` deliberately emits JSON objects per line.

Use `logs --tail 10`, `logs --since 30m --level WARNING`, `logs --follow`, `debug on --duration 15m`,
and `debug off`. Debug off restores the configured baseline, including a DEBUG baseline. Levels
are runtime overrides, not YAML/dotenv edits. Logs are application-owned JSONL, not system-wide
journald or HCI captures. Secrets/control characters are sanitized; raw BLE bytes are DEBUG-only.
A file failure exposes degraded state and a bounded ring/stderr fallback, with explicit lost history.
No log-download path, shell or remote configuration editor exists.

## History and database handling

History records actual receive timestamps from new live notifications at the configured interval,
plus the first sample of a new continuity segment. Equal new readings are recorded; the same cached
notification is not re-stamped. Stale/offline periods produce no rows. Gaps/storage failures/clock
jumps start new segments; charts must not join them. No retention, aggregation or raw-frame/log table.
DELETE journal, FULL synchronous, foreign keys and busy_timeout=5000 are verified. SQLite 3.37+
STRICT schema constraints reject inconsistent output bits and invalid values. A future/unversioned
nonempty/corrupt database is not replaced with an empty one. Storage failure leaves live telemetry
and eligible controls available, with bounded dropped work and retry/reopen health.

Backups remain an operator/LXC responsibility. Include application config/token, data/logs and the
Caddy persistent certificate store, and verify external mounts are included in infrastructure backups.
For a live SQLite snapshot, use its Online Backup API or SQLite-aware `.backup`:

```bash
sqlite3 /var/lib/mypowers/mypowers.db '.backup /chosen/backup/mypowers.db'
sqlite3 /chosen/backup/mypowers.db 'PRAGMA integrity_check;'
```

Never use plain `cp` for a SQLite database, including apparently stopped instances: committed WAL
content can exist. Migration/restore experiments must use a consistent snapshot in a separate
temporary directory and pass `PRAGMA integrity_check`; never experiment on the original database.

## Lifecycle and resources

One daemon owns one adapter/station. Imports do no I/O. Lifespan starts supervised tasks without
waiting for RF acquisition. The event loop owns BLE/state/commands; aiosqlite uses a worker thread,
and JSONL/query workers are bounded. Notification callbacks do no SQL/file/network-client waits.
Unexpected essential-task termination appears as unhealthy diagnostics and revokes mutations.
SIGTERM/SIGINT stop new mutations, cancel pending operations with definite/uncertain outcomes,
release BLE, flush bounded queues, close DB/logs and release the lock. Transport cleanup is 5s;
Uvicorn graceful shutdown is 10s, supervisor examples allow 15s. No output reset occurs on exit.

The [validation report](validation/IMPLEMENTATION_VALIDATION.md) records actual latency, RSS, CPU,
continuity and versions. Resource targets are measurements, not firmware/platform guarantees.
