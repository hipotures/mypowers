# Configuration

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
