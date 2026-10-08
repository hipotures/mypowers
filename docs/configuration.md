# Configuration

Battery alerts run in the daemon with or without a TUI. Configure the initial rule in YAML:

```yaml
battery_alert:
  enabled: true
  threshold_percent: 20
  hysteresis_percent: 5
  min_notification_interval_minutes: 10
```

Settings → Alerts edits and saves this rule in SQLite; saved settings override the YAML defaults.
ALERT begins at battery ≤ threshold; recovery occurs at battery ≥ threshold + hysteresis.
Hysteresis is at least one percentage point, and the recovery threshold must not exceed 100%.
Only fresh LIVE telemetry changes the rule or releases a pending notification.
During cooldown only the latest logical state remains pending. A state already notified is never
sent again: low → recovered → low within cooldown produces no second low message.
Pending recovery reports the current battery percentage when it is sent.
Logical state, last notified state and timestamps persist in `mypowers.db`, independently of
whether telemetry history recording is enabled. State changes and delivery outcomes enter logs.

Set `MYPOWERS_TELEGRAM_BOT_TOKEN` and `MYPOWERS_TELEGRAM_CHAT_ID` in the server `.env`.
Both must be supplied together. They are omitted from check-config, API responses and logs.
The bot must be allowed to send to that chat. Settings → Notify → Send test message sends a
fixed test through Telegram's [sendMessage API](https://core.telegram.org/bots/api#sendmessage)
without changing the battery rule or its cooldown. Credentials are never entered in the TUI.
Without credentials, rule transitions are still logged, but no Telegram messages are sent.

Delivery attempts are reserved durably before contacting Telegram. A failed or uncertain attempt
retains only the current state and waits at least the cooldown (30 seconds when cooldown is shorter)
before retrying. Telegram and SQLite cannot be committed atomically: a crash after external delivery
but before recording its confirmation can cause a later retry, although never an immediate one.

Precedence is CLI overrides, process environment, dotenv, selected YAML, then validated defaults.
Server, CLI and TUI read `.env` from the current directory when `--env-file` is omitted.
An explicit `--env-file PATH` replaces that selection; files are not merged and parent directories
are not searched. An absent default `.env` uses process environment and defaults.
Select YAML with `--config`, `MYPOWERS_CONFIG`, or the XDG default
`$XDG_CONFIG_HOME/mypowers/config.yaml` (`~/.config/mypowers/config.yaml` fallback). An explicitly
selected missing file fails; an absent default file uses defaults. Dotenv is parsed as data with
interpolation disabled, never executed or discovered recursively. Unknown `MYPOWERS_*` names
are errors, including misspellings; client names and the known output-test opt-in are recognized.

Relative YAML paths are resolved against the YAML directory, dotenv paths against its directory,
and CLI/environment paths against the initial working directory. Paths become absolute once.
Duplicate/unknown YAML keys, unsafe tags, invalid MACs/types/ranges and nonfinite durations fail.
Error messages omit raw input and token values. `mypowersd check-config` prints
resolved settings and paths, excluding the token.

`config/production.example.yaml` is the complete supported YAML shape. Defaults: scan 20s, total
GATT/name/subscription setup 25s, first valid sample 5s, stale/write cutoff 3s, silence reconnect
10s, recording 10s, INFO logging, 10 MiB log files and five backups. The stale limit cannot exceed
3s. Retry delays are 2/5/10/20/30s plus at most 10% jitter; infrastructure checks use a slower wait.
These are application decisions, not measured firmware guarantees.

History records full telemetry only when values change. Unchanged values extend
their confirmed coverage once per UTC interval selected by `history.interval_seconds`.
Observation timestamps remain unchanged. Reconnection segments are recorded
immediately; unavailable telemetry leaves genuine gaps instead of repeating cached values.
See [history migration](history-migration.md) for the explicit version-1 conversion.

| Variable | Meaning |
|---|---|
| `MYPOWERS_ENV` | `development` default or `production` |
| `MYPOWERS_CONFIG` | YAML path |
| `MYPOWERS_BIND_HOST`, `MYPOWERS_PORT` | Backend listener, default loopback:8765 |
| `MYPOWERS_DATA_DIR` | `mypowers.db` location |
| `MYPOWERS_LOG_DIR` | Active/rotated `mypowers.jsonl` files |
| `MYPOWERS_RUNTIME_DIR` | Local adapter/station ownership lock |
| `MYPOWERS_LOG_LEVEL` | Baseline DEBUG/INFO/WARNING/ERROR |
| `MYPOWERS_AUTH_REQUIRED` | Defaults true; explicitly false only on development loopback |
| `MYPOWERS_API_TOKEN_FILE` | Preferred private token file (0600) |
| `MYPOWERS_API_TOKEN` | Alternative inline token; mutually exclusive with token file |
| `MYPOWERS_PUBLIC_URL` | Allowed public origin/hostname |
| `MYPOWERS_SERVER_URL` | Client HTTP(S) origin |
| `MYPOWERS_TIMEZONE` | Client IANA display zone, e.g. Europe/Warsaw |
| `MYPOWERS_CA_FILE` | Additional explicit CA trust for client TLS verification |
| `MYPOWERS_BACKEND` | BLE default; simulated only by explicit selection |
| `MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES` | Test-runner opt-in only; never expands application permissions |

Production requires auth, a non-placeholder token of at least 32 ASCII characters, real BLE and
loopback bind. Auth remains enabled behind Caddy. The production example uses `/etc/mypowers`,
`/var/lib/mypowers`, `/var/log/mypowers`, `/run/mypowers`. Deployer owns these paths and permissions.
Token generation uses 32 random bytes, refuses overwrite and never prints the secret:

```bash
mypowersd token generate --output /etc/mypowers/api-token
```

Installed user paths follow `$XDG_DATA_HOME/mypowers`, `$XDG_STATE_HOME/mypowers/logs`, and
`$XDG_RUNTIME_DIR/mypowers`. Fallback runtime is a private `run` directory under application state.
The repository dotenv explicitly uses `.local/dev` instead. Client-only configuration ignores
server settings and loads no YAML. URLs reject userinfo/path/query/fragment; redirects, ambient
HTTP proxy discovery and insecure TLS bypasses are disabled.

Command results/idempotency keys expire after one hour and are capped at 1,000. They are local
to one server UUID, not durable exactly-once storage. Streams cap 16 clients, 128 state events or
256 log records per client; slow clients close with retryable 1013. Incoming HTTP bodies and
client-to-server WS messages cap 16 KiB. The TUI and shared CLI client cap received WS frames
and complete messages at 32 KiB, allowing space for a log record's stream envelope. History pages cap 10,000,
log pages/tails 1,000, and query admission caps four per subsystem.
Produced JSONL log records also cap 16 KiB, including their newline. Oversized records
mark context as truncated and shorten text as needed to remain readable by the archive API.


## Persisted application preferences

The daemon stores mutable public preferences in SQLite `settings(key, value_json)`:
graph interval (10/30/60/3600 seconds), visualization (Sparkline/Chart), base scale
(100/300 W with automatic doubling), timezone (`system` or IANA), and log page size
(50/100/250/500/1000). These values are loaded by each TUI and survive daemon restarts.
Graph aggregation intervals are independent of `history.interval_seconds`, which
controls coverage checkpoints for unchanged telemetry. Preference storage works
when recording is disabled.

Read and update preferences through `GET` / `PUT /api/v1/settings`. Updates change
only supplied, validated fields. New preferences require a typed contract field
and UI editor, without changing the table. Unknown fields are rejected. Secrets
stay outside public mutable settings. YAML configures deployment, authentication
and hardware access. Retry, pause/resume and runtime DEBUG are actions rather than
persistent preferences. Explicit TUI timezone flags take precedence on startup.

## Bluetooth connection alerts

Settings → Alerts has a separate **Bluetooth connection** section. Its En/Dis
switch is independent of battery alerts. The daemon sends Telegram alerts for a
missing/off/blocked controller, loss of the S300 link, or unavailable fresh data.
It cannot distinguish S300 Bluetooth being switched off from station power loss,
range problems or another client solely from a failed scan; the message says so.

Configure the initial rule in server YAML:

```yaml
connection_alert:
  enabled: true
  outage_seconds: 60
  recovery_seconds: 15
  min_notification_interval_minutes: 10
```

All four values are editable in Settings and saved in SQLite. Saved settings
supersede YAML defaults. `outage_seconds` accepts 1–86400, `recovery_seconds`
1–3600, and the notification interval 0–1440 minutes. Recovery requires continuous
LIVE telemetry for the configured duration; any stale interval resets that timer.
The interval applies to both outage and recovery messages. Pending changes
collapse to the latest state, and a continuing outage is not repeatedly announced.
A failed or uncertain delivery waits at least 30 seconds and the configured interval
before retrying. Exactly-once delivery across a crash during Telegram delivery is
not possible; the attempt is reserved durably before contacting Telegram.

Intentional connection Pause, daemon shutdown and a disabled rule do not raise an
alert. Disabling clears the pending episode. Re-enabling starts a new outage delay.
No initial recovery message is sent unless an outage was successfully notified.
Outage start, delivery attempts and confirmed notification state survive restart;
recovery hysteresis is observed again after restart. The daemon creates the new
`connection_alert_state` table automatically on startup, preserving history and
battery alert state. History recording may be disabled without disabling alerts.
