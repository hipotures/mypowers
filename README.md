# MyPowers

MyPowers monitors one qualified ALLPOWERS S300 and explicitly controls its AC group, DC group
and common lamps. A foreground daemon owns BLE, records change-only SQLite history and serves an
HTTP/WebSocket API. The Rich CLI and Rust/Ratatui TUI use that same API locally or remotely.
Closing a frontend never stops collection or changes station outputs.

The qualified unit is `2A:02:01:48:6B:D0`, readable name `AP S300 V2.0`, using the Actions adapter
`F4:4E:FC:A1:CB:FF`. Adapter identifiers are resolved on every attempt; there is no fallback.
Only the eight experimentally qualified combined states are writable. Unknown firmware,
temperature, per-port data and lamp/SOS modes are not presented as supported capabilities.

## Development

Requires Linux, Python 3.12+, SQLite 3.37+, `uv`, and Rust/Cargo for the TUI. Real hardware additionally needs system
D-Bus/BlueZ and permission to use the intended controller. MyPowers installs no OS packages.

```bash
uv sync --locked --extra server --extra cli --group dev
# Copy the example once; keep local secrets/runtime files out of Git.
cp .env.example .env
uv run mypowersd check-config
uv run mypowersd
```

In another terminal:

```bash
uv run mypowers status
uv run mypowers status --json
cargo build --release --locked --manifest-path frontends/tui/Cargo.toml
install -m 755 frontends/tui/target/release/mypowers-tui .venv/bin/mypowers-tui
uv run mypowers-tui
uv run mypowers tui
```

The development example explicitly disables authentication on loopback and uses the real BLE
backend. To use the simulated backend, explicitly set `MYPOWERS_BACKEND=simulated` or pass
`mypowersd --backend simulated`; it is never selected on BLE failure. Production refuses it.
Development data/log/runtime directories are `.local/dev/data`, `.local/dev/logs`, `.local/dev/run`.
Server, CLI and TUI read `.env` in the current directory when `--env-file` is omitted.
Pass `--env-file PATH` to select another file; parent directories are not searched.
Process environment overrides dotenv values. Installed user defaults follow XDG directories.

```bash
# Real output changes: run only when you intend to change connected loads.
uv run mypowers ac on
uv run mypowers dc off
uv run mypowers light on
uv run mypowers connection pause
uv run mypowers connection resume
uv run mypowers logs --tail 10
uv run mypowers logs --follow
uv run mypowers debug on --duration 15m
uv run mypowers debug off
```

Use `a`/`d`/`l` for AC/DC/lamps, Tab/Shift-Tab and Enter/Space, mouse clicks for controls,
F3 to open logs, `r` for retry and `p` for pause/resume on the dashboard.
Use `s` for the read-only Settings / Diagnostics modal.
In Logs, use `b` for runtime DEBUG, `f` to filter, Left/Right for days, Home for the
selected day's beginning, and End to fetch today/live. `?` opens contextual help.
Only Esc, `q`, and Ctrl+Q are global: Esc closes a modal, `q` confirms exit, and
Ctrl+Q exits immediately. Typed Ctrl+C/Ctrl+Z are ignored; external SIGINT/SIGTERM
restore the terminal. `--no-mouse`, `--no-color`, `NO_COLOR`, `--timezone Europe/Warsaw`, and `--utc`
are supported. Paste cannot trigger controls. Stale/pending states disable output intentions.
Unknown/unconfirmed outcomes show concise feedback and are never automatically replayed;
command IDs remain in the API and logs.

## Verification and installation

Generate deterministic SVG previews from the actual Ratatui renderer, without
running a daemon or terminal: `cargo xtask ui-snapshots`. Files are written to
[`artifacts/ui`](artifacts/ui/README.md).

```bash
uv run ruff check src frontends tests
uv run ruff format --check src frontends tests
uv run mypy src frontends
cargo build --locked --manifest-path frontends/tui/Cargo.toml
uv run pytest -m 'not hardware'
uv run pytest -m 'not hardware' --cov --cov-report=json:coverage.json
uv run python tests/check_coverage.py coverage.json
uv build
uv run python tests/verify_wheel.py dist/mypowers-0.1.0-py3-none-any.whl
```

The separate research baseline is documented in the validation report. Offline tests always
use fakes and temporary storage. Real hardware acceptance requires a separate explicit runner;
see [hardware preparation](tests/hardware/README.md). Output-test opt-in requires operator-established
safe loads and ordinary common-lamp mode. A 0 W reading is not a safe-load declaration.

Install the Python wheel with `mypowers[server]` or `mypowers[cli]`.
Install the native TUI separately with `cargo install --locked --path frontends/tui`.
See [TUI usage and verification](frontends/tui/README.md).
The server needs no Rich; clients need no BLE/server/storage libraries. See [deployment](deploy/README.md)
for locked exports, installation, supervisors and Caddy. No process creates a venv at runtime.

- [API and streams](docs/api.md), [generated OpenAPI](docs/openapi.json)
- [Configuration](docs/configuration.md), [operations](docs/operations.md)
- [Change-only history and explicit SQLite conversion](docs/history-migration.md)
- [Implementation validation and acceptance results](docs/validation/IMPLEMENTATION_VALIDATION.md)
- [Preserved exact-unit research](docs/research/s300/README.md)

The application follows the supplied PRD, preserved as [docs/PRD.md](docs/PRD.md). Research remains unchanged.
