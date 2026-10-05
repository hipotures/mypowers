# MyPowers — Product and Implementation Requirements

**Version:** 1.0  
**Prepared:** 2026-10-04  
**Intended repository location:** `docs/PRD.md`  
**Repository:** `hipotures/mypowers`  
**Reviewed baseline:** `52dc2033d9975b15598b36402691c2a92aafdfc8` (`main`)  
**Product:** MyPowers  
**Server executable:** `mypowersd`  
**CLI executable:** `mypowers`  
**TUI executable:** `mypowers-tui`; also available through `mypowers tui`  
**Production hostname:** `mypower.efez.net`

## Implementation directive

Implement this document end to end in the existing repository. Deliver a working server, a one-shot Rich CLI, an interactive Rust/Ratatui TUI, automated tests, installation artifacts, and an evidence-based validation report. Do not stop after scaffolding, a design proposal, a read-only demo, or the backend alone. AC, DC-group, and common-lamp control are required in the first release, not deferred features.

The implementation environment is expected to have the station powered on with Bluetooth enabled. Use the available hardware for read validation and the authorized, reversible output tests described in Section 16. Do not assume that the presence of Bluetooth alone establishes that connected loads are safe to interrupt. Do not disable host services, change radio blocks, or alter Proxmox infrastructure to manufacture failures; simulate those failures at the transport boundary instead.

Proceed independently through the implementation milestones. Make routine implementation decisions within the constraints below, run tests, fix failures, and continue. Do not ask the user to choose a Python framework, endpoint naming convention, directory layout, or database schema: these decisions are specified here. Report actual external blockers precisely without substituting simulated results for hardware validation or weakening safety checks.

This document is the application specification. The research bundle is hardware evidence, not the application architecture. Nothing in this PRD claims that implementation or production deployment tests have already passed.

---

## Contents

1. Scope and fixed decisions
2. Evidence, precedence, and limitations
3. Repository and packaging
4. Architecture and process lifecycle
5. Configuration, environments, and paths
6. Hardware protocol and capability boundaries
7. Connection lifecycle and freshness
8. Control execution and confirmation
9. State model and time semantics
10. SQLite telemetry history
11. Structured logs and remote diagnostics
12. HTTP and WebSocket contract
13. CLI requirements
14. Ratatui TUI requirements
15. Deployment and security
16. Implementation sequence and validation
17. Acceptance checklist and handover
18. Engineering reference notes

---

## 1. Scope and fixed decisions

### 1.1 Product objective

A small Linux service owns the connection to one ALLPOWERS S300, publishes current measurements and connection health, records periodic telemetry history, and executes verified output commands. Local or remote clients use exactly the same authenticated HTTP JSON API and WebSocket streams. The station's vendor cloud and phone application are not required during normal operation.

The server must continue to serve status, history, and available application logs when the station is unavailable. Closing a frontend must not stop collection or change any station output.

### 1.2 Release scope

| Deliver in this release | Explicitly not part of this release |
|---|---|
| Persistent BLE connection management and recovery | Docker, Portainer, or Proxmox/HomeStack provisioning |
| Validated telemetry and all eight qualified AC/DC/common-lamp combinations | Other device models or an operational multi-device manager |
| HTTP JSON API, WebSocket state and log streams | Vendor cloud, MQTT, a Home Assistant integration, or client-facing D-Bus |
| SQLite periodic telemetry storage and bounded raw-history queries | Retention, hourly/daily rollups, table partitioning, or energy-yield analytics |
| YAML configuration plus explicit `.env`/environment overrides | Configuration editors or a remote shell |
| JSONL application logs, rotation, remote tail/filter/follow, runtime log level | Logs in SQLite, system-wide journal access, or arbitrary file downloads |
| Rich one-shot CLI and full-screen, mouse/keyboard Rust/Ratatui TUI | Textual, a browser frontend, or a working GNOME extension |
| Small live power trends and basic diagnostics in TUI | An extensive charting/analysis application |
| Deployment examples, verification commands, and tests | Automatic installation of OS packages, SSH, DNS changes, or certificate-provider selection |
| `frontends/gnome/` and `frontends/web/` reserved with explanatory READMEs | Placeholder controls advertised as working capabilities |

“Not part of this release” does not mean that architecture may prevent a later implementation. It means no speculative implementation or extra runtime service is required now.

### 1.3 Technology decisions

| Concern | Decision |
|---|---|
| Runtime | Python 3.12 or newer; test Python 3.12 and 3.14 in CI |
| Concurrency | `asyncio`; one server process and one event loop owning BLE |
| BLE | Bleak **3.0.2**, with **dbus-fast 3.1.2** as the initial tested baseline |
| Server framework | FastAPI, Pydantic v2, Uvicorn; exactly one worker |
| HTTP clients | `httpx`; use asynchronous clients in TUI |
| WebSocket transport | `websockets`, with versions resolved and locked during implementation |
| Database | Standard Python SQLite support plus `aiosqlite`; explicit SQL, no ORM |
| Config | PyYAML safe loading, Pydantic validation, python-dotenv for explicitly selected env files |
| Rendering | Rich for CLI; Rust/Ratatui for TUI |
| TUI input | Crossterm keyboard, mouse, resize and bracketed-paste events |
| Logging | Standard `logging`, structured JSONL formatter, bounded queued file writer |
| Packaging | One Python distribution, explicit frontend package mappings, optional extras, Hatchling |
| Development | `uv`, committed `uv.lock`, local `.venv` |
| Production | Foreground process in Proxmox LXC, supervised by the environment's service manager |
| Public transport | Caddy terminates HTTPS/WSS; backend remains loopback HTTP/WS |

Pin the other direct/transitive dependencies in `uv.lock` after resolving compatible stable versions. Do not claim that “latest” versions have been tested. Do not change the BLE baseline merely for uniformity; a necessary compatibility or security update must be recorded and revalidated on hardware.

## 2. Evidence, precedence, and limitations

### 2.1 Required inputs

Read these repository-local files before implementing:

| ID | Repository path | Role |
|---|---|---|
| R1 | `docs/research/s300/README.md` | Import provenance, source priorities, known historical link issue |
| R2 | `docs/research/s300/ALLPOWERS_S300_PROTOCOL.md` | Authoritative exact-unit read/write protocol; Section 3 contains the eight-state matrix |
| R3 | `docs/research/s300/ALLPOWERS_S300_CONNECTION_VALIDATION.md` | Observed failure modes, contention, and recovery evidence |
| R4 | `docs/research/s300/control_allpowers.py` | Maintained reference parser and eight-profile control probe |
| R5 | `docs/research/s300/test_control_allpowers.py`, `test_s300_matrix.py`, `test_s300_connection_lab.py` | Existing offline guards and regression checks |
| R6 | `docs/research/s300/allpowers_s300_evidence/` | Original captures, vectors, manifests; especially `matrix/` and `connection/` |
| R7 | `docs/research/s300/ALLPOWERS.md`, `BLUETOOTH.md`, `read_allpowers.py` | Investigation history and passive-demo reference |
| R8 | `docs/research/s300/ALLPOWERS_TUI_PRD.md` | Historical visual inspiration only; NOT the application specification |

The repository import README reports 35 passing research tests. Re-run them; do not merely repeat that historical result in the new validation report. The matrix evidence contains eight complete states and eight directed Gray-cycle transitions repeated three times. It is not 24 distinct hardware-tested directed transitions. Other single-switch directed edges have offline encoding coverage. [R1, R5, R6]

### 2.2 Precedence rules

- This PRD controls product scope, API, packaging, persistence, UI, deployment boundaries, and application tests.
- R2/R3 control what the hardware evidence supports. This PRD does not authorize new BLE capabilities.
- Within the historical bundle, the complete eight-state qualification and maintained probe supersede earlier four-frame restrictions. Historical logs and failed candidate experiments remain evidence, not command allowlists.
- Preserve `docs/research/s300/` unchanged. Add new application evidence under `docs/validation/`; use new test fixtures outside the research directory when adaptation is necessary, retaining provenance.
- Do not make production code import research modules or launch a research script and parse its output.

**Known source inconsistency:** R2's physical-lamp discussion retains an older sentence saying initially lit lamps block the probe. R2's later qualified matrix, maintained-probe section, and current R4 accept all eight qualified flag states. The application follows that later eight-state qualification for common-lamp on/off, while making no claim to preserve an unobserved individual-lamp/SOS mode. Do not “fix” the archived document in place.

**Known reference issue:** R1 identifies one historical relative link in the connection-evidence README that points one directory too high. Use R3's actual path; do not treat the missing target of that link as missing research.

### 2.3 What is not established

Hardware and firmware revision numbers, numerical temperature, BLE thermal/fault codes, per-port readings, individual USB switching, separate lamp modes, frequency in measured Hertz, ECO, charging modes, timers, and Bluetooth-off commands remain unverified. Do not create working endpoints, fake values, or controls for them. `AP S300 V2.0` is a read name, not a measured firmware revision. [R2]

The connection experiments were performed in a QEMU guest. They do not establish working Bluetooth exposure inside the future LXC. Live BlueZ service failure, some permissions failures, and a naturally silent initial correct subscription were not reproduced; cover them with fault-injected tests and label their validation method honestly. [R3]

## 3. Repository and packaging

### 3.1 Required organization

Preserve the existing license and research. Use this structure; individual modules may be split when they acquire a clear responsibility, not merely to fill folders:

```text
mypowers/
├── pyproject.toml
├── uv.lock
├── .python-version
├── .gitignore
├── .env.example
├── .env.production.example
├── .env.client.example
├── README.md
├── LICENSE
├── src/mypowers/
│   ├── __init__.py
│   ├── contracts/          # Transport DTOs and stable enums; no BLE imports
│   ├── client/             # HTTP/WS client for the Python CLI
│   ├── core/               # Immutable state, freshness, commands, events
│   ├── protocol/           # Pure frame decoding/encoding and qualified profile
│   ├── bluetooth/          # Bleak/BlueZ adapter and session boundary
│   ├── storage/            # SQLite repository and schema migrations
│   ├── diagnostics/        # Structured records, rotation, remote log access
│   ├── api/                # FastAPI routes, auth, WebSocket adapters
│   ├── config/             # Typed config loading and path resolution
│   └── daemon/             # Entrypoint, composition, ownership, shutdown
├── frontends/
│   ├── cli/src/mypowers_cli/
│   ├── tui/src/  # Rust crate with Cargo.toml and Cargo.lock
│   ├── gnome/README.md
│   └── web/README.md
├── config/
│   ├── development.yaml
│   └── production.example.yaml
├── deploy/
│   ├── README.md
│   ├── systemd/mypowers.service
│   ├── openrc/mypowers
│   └── caddy/
│       ├── Caddyfile.production.example
│       └── Caddyfile.local-test
├── tests/
│   ├── unit/
│   ├── integration/
│   ├── terminal/
│   ├── hardware/
│   └── fixtures/s300/
├── docs/
│   ├── PRD.md
│   ├── api.md
│   ├── configuration.md
│   ├── operations.md
│   ├── validation/
│   └── research/s300/      # Existing bundle, unchanged
└── .github/workflows/ci.yml
```

### 3.2 Python distribution and native TUI

Use one repository containing the `mypowers` Python distribution and the native `mypowers-tui` Rust crate. Physically separated frontend code must be installed correctly; no `sys.path` mutation or dependency on launching installed programs from the repository root.

Hatchling supports explicit package source mappings [E5]. Configure and verify a wheel containing the Python packages, for example:

```toml
[tool.hatch.build.targets.wheel]
packages = [
  "src/mypowers",
  "frontends/cli/src/mypowers_cli",
]
```

Provide Python extras `server` and `cli`, and install the Rust TUI with `cargo install --locked --path frontends/tui`. The base package contains only shared configuration/contract support and lightweight imports. The `server` extra includes BLE, FastAPI/Uvicorn, SQLite adapter, YAML, and WebSocket server dependencies. `cli` includes Rich and HTTP client dependencies. The native TUI owns its HTTP/WS and TLS client; `mypowers tui` executes the installed native binary.

A server-only installation must not require Rich. A client-only installation must not require Bleak, BlueZ, FastAPI, or the SQLite adapter. Shared `contracts` and `client` modules must not import server infrastructure. Entrypoints for absent extras return a short installation hint, not an import traceback.

The research bundle, test captures, local databases, credentials, logs, and `.env` files must not ship in the runtime wheel. Test the wheel from outside the source checkout. Development uses `uv`; production may use an appropriately provisioned interpreter without a venv. The program must never create a venv or install packages at runtime.

## 4. Architecture and process lifecycle

### 4.1 Dependency direction

```text
CLI / Ratatui TUI / later GNOME and Web
                 |
          HTTP JSON + WS
                 |
       FastAPI transport/auth
                 |
             Core service
          /       |        \
      BLE port  History port  Diagnostics port
         |          |             |
  Bleak + BlueZ   SQLite      JSONL / log streams
         |
        S300
```

Protocol functions are pure. The core uses injected clock/transport/storage interfaces and can be tested without hardware. API handlers validate/authorize requests and call the core; they do not construct BLE frames. Frontends render DTOs and submit intentions; they do not implement device safety rules themselves.

Do not introduce a message broker, plugin framework, generic device discovery platform, extra microservices, or a dependency-injection container. Small typed interfaces and explicit composition are sufficient.

### 4.2 Ownership and startup

**PROC-01:** Exactly one daemon process owns this configured device. Uvicorn has one worker. Reject unsupported multi-worker configuration. No automatic reload with the real BLE backend; reload may be used only with the explicitly selected simulated backend.

**PROC-02:** Acquire a nonblocking OS file lock in the configured runtime directory, keyed by adapter and station identity, before starting BLE. A second instance using that resource/runtime directory exits clearly. Keep the descriptor for process lifetime. Do not use a PID-file existence check as a lock. This is local process coordination, not protection against another host, phone, or user with a different runtime directory.

**PROC-03:** Use FastAPI lifespan to own startup/shutdown [E1]. Importing modules must not scan Bluetooth, open the database, or start threads. Start the connection supervisor as a managed task, then make the API available without waiting up to 20/25 seconds for radio operations. Device absence must not block HTTP startup.

**PROC-04:** Start as a foreground process. The external supervisor owns autostart/restart. Production examples may use systemd or OpenRC. The application must not call `systemctl`, `rc-service`, `apt`, `apk`, Proxmox APIs, or SSH to manage its environment.

### 4.3 Bounded work

The event loop owns state updates and command scheduling. `aiosqlite` uses a worker thread [E6]; queued logging/file queries may also use bounded off-loop workers. This is intentional and does not require a multithreaded BLE design. Do not claim that the entire application has zero threads.

BLE callbacks only validate, timestamp, update compact state, and publish bounded events. No file/SQL/network-client waits inside a notification callback. Use a latest-sample slot rather than an unbounded queue of redraws. Bound history jobs, log queues, WebSocket clients, per-client queues, query result sizes, and retained command results.

Unexpected exceptions in essential background tasks must be observed, logged, and reflected in health or cause an orderly nonzero exit. Never leave HTTP reporting a healthy service while an essential task has silently died.

### 4.4 Shutdown

SIGINT and SIGTERM stop admission of new mutations, stop collection scheduling, terminate clients/streams, clean up the BLE session, flush already queued history/logs within deadlines, close the database, release the lock, and exit. Overall target: 10 seconds under normal conditions. Document any transport cleanup timeout.

Do not change AC/DC/lamps as a daemon startup/shutdown action. Normal application commands persist as device state; their effect is not rolled back when the HTTP client or TUI exits. Guarded restoration belongs to the explicit hardware-test harness, not every production command.

## 5. Configuration, environments, and paths

### 5.1 Sources and precedence

Use the same application in development and production. `MYPOWERS_ENV` is `development` (the default) or `production`; it selects safe defaults and validation rules, never a different protocol implementation. Authentication defaults to enabled; the development example disables it explicitly on loopback.

Precedence, highest first:

1. Explicit command-line overrides.
2. Existing process environment.
3. The file explicitly selected with `--env-file PATH`.
4. The selected YAML configuration.
5. Documented defaults.

Do not recursively discover `.env` files in parent directories. No env file is loaded unless selected. The documented local invocation passes `--env-file .env`. Parse dotenv as data, never as shell code. Environment must win over file contents [E7]. Do not log secret values when configuration fails.

Select the YAML path using `--config`, `MYPOWERS_CONFIG` from environment/dotenv, or the default config path. An explicitly selected missing file is an error. If the default XDG config file is absent and no file was explicitly selected, use validated defaults; missing credentials still fail when auth is required. Unknown YAML keys, duplicate YAML keys, invalid types, nonfinite durations, invalid MAC addresses, and invalid ranges fail validation with actionable messages. Use safe YAML construction; do not execute tags. Maintain an explicit registry of recognized `MYPOWERS_*` names so misspellings are reported; recognize client-only names without applying them to server settings.

Relative paths in YAML resolve against that YAML's directory; relative paths in dotenv resolve against the dotenv's directory; relative explicit CLI/environment paths resolve against the initial working directory. Convert to absolute paths once. Do not depend on a later working-directory change.

### 5.2 Application YAML

Use this as the production example; these timing values are product defaults, not firmware guarantees:

```yaml
schema_version: 1

device:
  address: "2A:02:01:48:6B:D0"
  expected_name: "AP S300 V2.0"
  profile: "s300-v2-qualified-2026-10-04"

bluetooth:
  adapter_address: "F4:4E:FC:A1:CB:FF"
  scan_timeout_seconds: 20
  setup_timeout_seconds: 25
  first_sample_timeout_seconds: 5
  stale_after_seconds: 3
  reconnect_after_seconds: 10

history:
  enabled: true
  interval_seconds: 10

logging:
  level: "INFO"
  max_file_bytes: 10485760
  backup_count: 5

api:
  bind_host: "127.0.0.1"
  port: 8765
  auth_required: true
```

No retention or aggregate configuration. Writable controls are not supplied as arbitrary YAML command bytes. The protocol allowlist is reviewed application code, not a user-editable escape hatch.

### 5.3 Deployment variables

Implement and document at least:

| Variable | Meaning |
|---|---|
| `MYPOWERS_ENV` | Environment name |
| `MYPOWERS_CONFIG` | Server YAML path |
| `MYPOWERS_BIND_HOST`, `MYPOWERS_PORT` | Backend bind address and port |
| `MYPOWERS_DATA_DIR` | Directory containing `mypowers.db` |
| `MYPOWERS_LOG_DIR` | Directory containing rotated `mypowers.jsonl` |
| `MYPOWERS_RUNTIME_DIR` | Lock/runtime directory |
| `MYPOWERS_LOG_LEVEL` | Baseline log-level override |
| `MYPOWERS_AUTH_REQUIRED` | Boolean auth policy; production must be true |
| `MYPOWERS_API_TOKEN_FILE` | Server/client token file, preferred over an inline secret |
| `MYPOWERS_API_TOKEN` | Alternative direct secret; mutually exclusive with token file |
| `MYPOWERS_PUBLIC_URL` | External origin for deployment validation and allowed browser origin; not the bind address |
| `MYPOWERS_SERVER_URL` | Client target, e.g. production HTTPS URL |
| `MYPOWERS_TIMEZONE` | Client display timezone, e.g. `Europe/Warsaw` |
| `MYPOWERS_CA_FILE` | Optional explicit client CA bundle for a private/local TLS test; verification stays enabled |
| `MYPOWERS_BACKEND` | `ble` by default; `simulated` only when explicitly requested |

Development `.env.example`:

```dotenv
MYPOWERS_ENV=development
MYPOWERS_CONFIG=./config/development.yaml
MYPOWERS_BIND_HOST=127.0.0.1
MYPOWERS_PORT=8765
MYPOWERS_DATA_DIR=./.local/dev/data
MYPOWERS_LOG_DIR=./.local/dev/logs
MYPOWERS_RUNTIME_DIR=./.local/dev/run
MYPOWERS_AUTH_REQUIRED=false
MYPOWERS_SERVER_URL=http://127.0.0.1:8765
MYPOWERS_TIMEZONE=Europe/Warsaw
MYPOWERS_BACKEND=ble
```

Production `.env.production.example`:

```dotenv
MYPOWERS_ENV=production
MYPOWERS_CONFIG=/etc/mypowers/config.yaml
MYPOWERS_BIND_HOST=127.0.0.1
MYPOWERS_PORT=8765
MYPOWERS_DATA_DIR=/var/lib/mypowers
MYPOWERS_LOG_DIR=/var/log/mypowers
MYPOWERS_RUNTIME_DIR=/run/mypowers
MYPOWERS_AUTH_REQUIRED=true
MYPOWERS_API_TOKEN_FILE=/etc/mypowers/api-token
MYPOWERS_PUBLIC_URL=https://mypower.efez.net
MYPOWERS_BACKEND=ble
```

Client `.env.client.example` uses `MYPOWERS_SERVER_URL=https://mypower.efez.net`, a local `MYPOWERS_API_TOKEN_FILE`, and optionally `MYPOWERS_TIMEZONE=Europe/Warsaw`. It must not contain the DNS-provider credential.

Recognize `MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES` as a test-tool-only variable, not an application capability or protocol override. Server configuration may ignore this known test variable without treating it as a misspelling.

Production must refuse disabled auth, a missing/placeholder token, a simulated backend, and a non-loopback backend bind for this release's same-LXC Caddy topology. Development with disabled auth must also be loopback-only. Changing an environment flag must never expand BLE permissions or silently select fake data.

### 5.4 Paths and ownership

For an installed user-mode development service, honor XDG configuration/data/state/runtime locations, falling back to `~/.config/mypowers/`, `~/.local/share/mypowers/`, and `~/.local/state/mypowers/` as appropriate. Use `$XDG_RUNTIME_DIR/mypowers/` for a user runtime directory when available, otherwise a private mode-0700 `run/` directory under the application state directory. The repository's explicit development dotenv overrides these with `.local/dev/` paths.

Production defaults are explicitly supplied by the deployment example above. The deployer creates/owns directories for the selected service account. The code must work as an unprivileged user with access to BlueZ and those directories; it must not require root. A deployer choosing root inside its LXC does not change application behavior. Do not demand or create a production venv.

Ignore `.venv/`, `.local/`, local `.env*` secrets, SQLite/journal files, runtime logs, caches, and validation working directories in Git, while explicitly retaining `.env.example`, `.env.production.example`, and `.env.client.example` through individual ignore exceptions. Do not use a global `*.jsonl` ignore rule that hides research evidence.

### 5.5 Local management commands

Provide `mypowersd --env-file PATH` to run, `mypowersd check-config --env-file PATH` for local validation with redacted resolved paths/settings, and `mypowersd token generate --output PATH` to create an unpredictable token using at least 32 random bytes. Token generation is local, refuses overwrite, and creates a mode-0600 file. It does not contact Bluetooth or print the token into a log.

## 6. Hardware protocol and capability boundaries

The following is a compact implementation contract derived from R2/R4, not fresh protocol research.

### 6.1 Identity and services

Station: `2A:02:01:48:6B:D0`, expected name `AP S300 V2.0`. Preferred adapter: Actions `F4:4E:FC:A1:CB:FF`, USB `10d7:b012`. Resolve its current `hciN` by MAC each attempt. Never silently choose CSR/Intel or a default adapter.

| Role | Full UUID |
|---|---|
| Service | `0000fff0-0000-1000-8000-00805f9b34fb` |
| Status notify | `0000fff1-0000-1000-8000-00805f9b34fb` |
| Output writes | `0000fff2-0000-1000-8000-00805f9b34fb` |
| Readable name | `00002a00-0000-1000-8000-00805f9b34fb` |

Validate service membership and characteristic properties. Read the device name and remove trailing NUL padding before comparison. Address/name checks select a tested device; they are not cryptographic authentication or firmware identification. The configured address is not permanently embedded in generic transport logic, but the initial write profile is qualified only for this unit. Selecting another address must not silently qualify another device for writes.

Use discovered `BLEDevice` and explicit `bluez={"adapter": current_hci}`. Stop scanning before connection. Do not filter solely by advertised service UUIDs: this station was found without a service list. Do not start polling writes, pairing, trust, or automatic adapter power cycling. [R2, R3]

### 6.2 Status frames

A supported notification is exactly 16 bytes:

| Bytes | Meaning |
|---|---|
| 0–1 | `a5 65` |
| 2–4 | Qualified status envelope `b1 00 01` |
| 5 | Payload length `08`; total length `8 + frame[5]` |
| 6 | Status command `01` |
| 7 | Status flags |
| 8 | Battery integer 0–100 |
| 9–10 | Aggregate input watts, unsigned big-endian uint16 |
| 11–12 | Aggregate output watts, unsigned big-endian uint16 |
| 13–14 | Station remaining-time estimate, unsigned big-endian uint16 minutes |
| 15 | XOR checksum; XOR of all 16 bytes equals zero |

Match R4's strict qualified decoder, including envelope validation and rejection of previously unobserved RX bit 7. Unqualified lower flag combinations may be read diagnostically when the structural decoder accepts them, but must not authorize writes. Distinguish a malformed packet from a structurally valid unqualified write profile.

Reject truncated, concatenated, differently shaped, invalid-checksum, and out-of-range packets. Do not add speculative stream reassembly. Count rejected packets; log raw bytes only at DEBUG. An invalid packet must never replace the last valid numeric sample with zeros or reset its freshness clock.

Golden example: `a565b1000108010e63003700190190ab` decodes to 99%, 55 W input, 25 W output, 400 minutes, AC true, DC false, light false, flags 14.

### 6.3 Qualified output frames

```text
A5 65 00 B1 01 01 00 FLAGS XOR
```

The write is a complete combined state, not a hardware toggle. XOR covers the first eight bytes. Send to the control UUID with explicit `response=False`.

| AC | DC | Common lamps | Complete RX | TX flags | Exact TX frame |
|---|---|---|---|---|---|
| off | off | off | `0c` | `18` | `a56500b10101001869` |
| on | off | off | `0e` | `1a` | `a56500b10101001a6b` |
| off | on | off | `0d` | `19` | `a56500b10101001968` |
| on | on | off | `0f` | `1b` | `a56500b10101001b6a` |
| off | off | on | `1c` | `38` | `a56500b10101003849` |
| on | off | on | `1e` | `3a` | `a56500b10101003a4b` |
| off | on | on | `1d` | `39` | `a56500b10101003948` |
| on | on | on | `1f` | `3b` | `a56500b10101003b4a` |

RX DC `01` maps to TX `01`; RX AC `02` maps to TX `02`; RX lamp `10` maps to TX `20`. Preserve RX `04/08` through TX `08/10`. Do not label RX `08` a verified buzzer status. Reject writes outside these eight complete RX and TX profiles; do not silently “repair” the selector/persistent bits by forcing a baseline.

### 6.4 Publicly exposed readings

Expose battery %, aggregate input/output W, the station time estimate, AC/DC/common-lamp booleans, connection diagnostics, last scan RSSI with its measurement age, and nominal specifications labeled as user-supplied metadata. Expose `status_flags` as diagnostic data, not a promise that every bit is understood.

Zero is a valid measurement, not a missing-value marker. `remaining_minutes=0` is displayed as `0h 00m`; do not infer a special meaning. Do not infer that `DC off` disables all USB-C ports. Do not present input watts as a calibrated irradiance/solar measurement, or input minus output as measured battery charging power. There is no calculated Wh/runtime/charge prediction in v1. [R2]

## 7. Connection lifecycle and freshness

### 7.1 Independent observations

Maintain separate connection phase, telemetry freshness, protocol qualification, and subsystem health. A single `connected` boolean is insufficient. In R3, the disconnect callback followed the last valid frame by 45.979 seconds after station Bluetooth was disabled. That is an observation from one trial, not a timeout to reproduce in the application.

Represent at least these phases/reasons:

| Phase/reason | Behavior and user-facing meaning |
|---|---|
| `starting` | Initializing local resources; no current telemetry yet |
| `system_bus_unavailable` | Cannot access the configured system bus; do not diagnose a missing USB adapter |
| `bluez_unavailable` | Bus is accessible but BlueZ is unavailable |
| `permission_denied` | Explicit permission failure; report the operation and actionable configuration hint |
| `adapter_missing` | Configured MAC unavailable to BlueZ; suggest USB, driver, or VM/LXC exposure checks |
| `adapter_off` | Present, powered off, radio block known clear |
| `adapter_blocked` | Locally observed rfkill block |
| `adapter_not_ready` | Not ready but available properties cannot distinguish off/block/driver state |
| `scanning` / `station_not_found` | Bounded discovery; absence is not a diagnosis of phone contention |
| `connecting` / `connection_failed` | GATT setup in progress or failed at a recorded stage |
| `waiting_for_telemetry` / `no_telemetry` | Expected subscription active but no valid sample by local deadline |
| `connected` | Local BLE session exists; consult separate telemetry freshness |
| `reconnecting` | Recovering after link loss, sustained silence, or a required command resynchronization |
| `cleanup_failed` | Old session not safely disposed; do not start another connection |
| `paused` | Collection intentionally released by the API; outputs are unchanged |

Telemetry states are `unknown`, `waiting`, `live`, `stale`, and `invalid`. The last valid sample remains separately available as last-known information. An invalid packet temporarily revokes write eligibility; a later qualified valid packet can restore it. Invalid traffic does not keep telemetry fresh.

### 7.2 Detection and recovery

**CONN-01:** Check system D-Bus, BlueZ presence, configured adapter objects, power, and accessible rfkill information before RF work. Do not parse localized exception strings as the only classifier. Use structured D-Bus error names and observed properties. If rfkill/sysfs or optional BlueZ properties are unavailable, report the limited diagnosis rather than inventing one.

**CONN-02:** Resolve MAC to current adapter/sysfs/rfkill identifiers on each attempt. `hciN`, USB device numbers, and rfkill indices are not durable IDs. Observe BlueZ owner/property/object changes where available; use a slow bounded recheck as fallback.

**CONN-03:** Scan for at most 20 seconds. Use the newly discovered object for a single, overall 25-second GATT/name/subscription setup deadline. Avoid a second implicit address scan. Discard expired cached objects; do not purge pairing/cache or change adapters to obtain different errors.

**CONN-04:** Wait at most 5 seconds for the first valid status after subscribing. Do not call the session live before it arrives. At an established sample age of 3 seconds or more, mark it stale and immediately revoke writes. If silence reaches 10 seconds, begin one serial cleanup/reconnect. A real disconnect or infrastructure loss triggers recovery immediately; it need not wait for the silence deadline.

The 3-second stale and 10-second recovery defaults deliberately separate rapid warning/write denial from a short recovery grace period. These are application decisions, not measured optimal firmware timings. The freshness limit that authorizes a write must never exceed 3 seconds, even if display/recovery settings are changed.

**CONN-05:** Radio failure backoff is 2, 5, 10, 20, then 30 seconds, capped at 30 seconds, with up to 10% jitter; reset after valid telemetry. Tests inject the clock/randomness. Missing/off/blocked infrastructure waits on relevant changes or a slow recheck, not repeated full RF scans. Explicit retry or relevant infrastructure recovery wakes the wait without creating an overlapping attempt.

**CONN-06:** Cleanup is bounded. Stop notifications when possible, disconnect, invalidate the session generation, and discard its cached state for write authorization. If cleanup cannot establish release, retain the old handle and enter `cleanup_failed`; retry cleanup/recheck rather than immediately creating a new client. Late callbacks from obsolete generations must not update the new session or confirm a new command.

**CONN-07:** Do not automatically power on/unblock an adapter, toggle station Bluetooth, restart BlueZ, pair/trust a device, or fall back to another controller. The service remains observable through HTTP when infrastructure is unavailable.

### 7.3 Accurate messages

Use stable machine-readable reason codes plus English messages and hints. Examples:

```text
Station not found. Check station power, Bluetooth and range; disconnect other apps.
Actions Bluetooth controller is unavailable. Check USB or VM/LXC device exposure.
Bluetooth connected, but no valid station data has arrived.
Station data is stale. Last valid update: 4.2 seconds ago.
Connection information expired; searching again.
```

Do not output “the phone is connected” as a diagnosis. No unique busy-client error was found. Do not equate `NotReady` with either rfkill or powered-off without additional observations. Do not diagnose overheating from an unexpected AC-off status. [R2, R3]

### 7.4 Explicit connection operations

Support authenticated pause, resume, and retry through the API and CLI. Pause disconnects the daemon's BLE session and disables autonomous reconnect so the phone can use the device; it does not turn off the station's radio or outputs. Resume restarts acquisition. Retry wakes pending discovery/recovery; a healthy session is not torn down unnecessarily. Repeated identical requests are harmless.

Reject connection pause while an output operation is in flight; do not interrupt a command silently. A forced remote reset/reboot endpoint is not required. Intentional pause is a runtime setting, not persisted automatically; a new daemon process starts acquisition normally.

## 8. Control execution and confirmation

### 8.1 Commands are requested states

Expose explicit desired booleans, such as AC off, rather than a blind toggle command. The TUI switch sends a desired state based on its displayed snapshot; a stale view must not overwrite a newer state unnoticed. Each API call changes one logical output. There is no arbitrary-frame, arbitrary-UUID, or multi-setting endpoint.

Maintain an `outputs_revision` scoped to `server_instance_id`. Advance it when complete observed flags change or when write eligibility is invalidated by a new session, stale/invalid state, or loss of connection. Do not advance it on every battery/watt/time-only update. A command includes the server instance and expected output revision seen by its client.

### 8.2 Serialized transaction

**CTRL-01:** Admit at most one output operation at a time. Reject a competing mutation with `command_busy`; do not build an unbounded queue of old intentions. Deduplicated retries of the same admitted request retrieve its existing result rather than hitting the busy check.

**CTRL-02:** Keep ownership for the entire operation: fresh read, qualification, frame construction, write, and confirmation. Merely locking `write_gatt_char` is insufficient for multiple API clients. The research helper's lock is not a ready-made concurrent transaction manager. [R2 Section 5]

**CTRL-03:** Under that ownership, require a newly received valid sample after operation admission, no more than 3 seconds old, from the active session. Validate the client revision, complete qualified profile, identity, characteristic, and connection again immediately before transmission. If output flags/revision change while waiting, reject with `state_conflict`; do not silently rebase the client's intention.

**CTRL-04:** Derive the complete target from the observed qualified state, modify only the requested bit, translate direction-specific masks, preserve RX `04/08`, and validate the resulting eight-state allowlist. If the requested value already matches a fresh qualified state, return `no_change` without a BLE write.

**CTRL-05:** Record command ID, source/target state, session generation, receive sequence cutoff, and local monotonic transmit time. Write once using `response=False`, with a 3-second transport deadline. No automatic retry, fallback frame, or guessed repair.

**CTRL-06:** Within 10 seconds after transmission, require two consecutive fresh status notifications from that same session, both later than the write cutoff, whose entire flags byte matches the expected target. Battery/power/time may vary. Source-state frames before the first target frame may arrive; unrelated flags, invalid data, a contradiction after the first target, disconnect, or stale telemetry terminates confirmation without success.

**CTRL-07:** A successful Bleak call is not application acknowledgment. `confirmed` means the requested complete state was subsequently observed twice. The protocol has no transaction ID or conditional update; even this confirmation does not prove causal attribution against physical-button interference. Do not claim atomicity against station buttons or a second actor. [R2]

### 8.3 Operation lifecycle

Use these statuses in the command DTO:

- `accepted`, `waiting_for_status`, `sent`: pending.
- `confirmed`: two qualifying post-write observations.
- `no_change`: fresh observed state already matched; no write.
- `rejected`: a precondition failed before any write.
- `failed`: a definite local failure before transmission was attempted.
- `unconfirmed`: a write may have reached the station but its result could not be established.

Return a reason code and last observed state where available. `unconfirmed` must not be presented as “the output definitely did not change.” Do not retry it automatically. Require clean session resynchronization before another write; reconnect and obtain two new qualified observations, without replaying the old command. Late matching frames do not retroactively confirm an operation in a different session.

Observed telemetry must continue updating while an operation is pending. Render observed state and pending desired state separately. Never overwrite observed values optimistically with the requested state; do not freeze all telemetry until the second confirmation either.

### 8.4 Deduplication and client disappearance

Mutation requests require an `Idempotency-Key` UUID. Store admitted operations/results in a bounded in-memory registry: at most 1,000 results, expiring after one hour; never evict an active operation. A repeated key with the same normalized body returns the same operation. A repeated key with different parameters returns `idempotency_conflict`.

The body includes the originating `server_instance_id`; a request from before a restart is rejected, preventing silent replay into a different server lifetime. Deduplication is not durable exactly-once delivery across process crashes. Clients must not automatically resubmit control requests after ambiguous HTTP errors, key expiry, or server restart. Fetch the operation when available and otherwise fetch status and report uncertainty.

An accepted operation belongs to the daemon, not to an HTTP handler task or WebSocket connection. Client disconnection does not cancel an already accepted operation, create a second attempt, or trigger restoration. Shutdown may leave a transmitted operation unconfirmed; log that distinction.

### 8.5 Never fight the station

Do not automatically re-enable an output after a physical-button change, spontaneous shutdown, reconnect, server restart, or suspected protection event. Record the new observation and expose it. This application is a monitor and explicit controller, not a desired-state enforcement loop.

## 9. State model and time semantics

### 9.1 Internal state

Use immutable, typed sample/command records. Keep separate concepts for:

- last valid telemetry sample and its source timestamps;
- current connection phase and reason;
- protocol qualification and control eligibility;
- desired connection state (`running` or `paused`);
- current pending operation and bounded recent operation results;
- history and logging health/counters;
- last scan RSSI with its own receive time;
- server identity, instance UUID, uptime, and environment.

An absent sample is null, not a sample full of zero/false fields. A last-known sample after reconnect or restart must never be mislabeled current. After restart, the latest database sample may be exposed explicitly as persisted last-known history, with no monotonic freshness or write authority.

### 9.2 Three different times

1. Persist server receive time as Unix epoch milliseconds in UTC. It is reception time, not a device-provided measurement clock.
2. Serialize times in API/log JSON as timezone-aware RFC 3339 UTC with `Z`, including milliseconds.
3. Use monotonic time for freshness, scheduling, deadlines, operation confirmation, and elapsed durations. Do not persist monotonic numbers as cross-reboot absolute timestamps. [E8]

Client presentation converts UTC to a selected IANA zone or local client time. Support `Europe/Warsaw` and an explicit UTC override. Convert each timestamp with timezone rules, not a hard-coded `+02:00` offset. Use `zoneinfo`; include `tzdata` in frontend dependencies where the OS may lack timezone data. [E9]

Named timezone support is not necessary for server storage: server UTC behavior is independent of LXC's configured display timezone. Host clock synchronization is an infrastructure responsibility. Detect significant wall-clock discontinuities against monotonic elapsed time, log them, and start a new history segment. Do not adjust the OS clock or assume `(device_id, timestamp)` is unique.

The API includes `server_time` and sample age calculated by the server. A client can advance the received age with its own monotonic clock; it must not infer freshness by subtracting unsynchronized desktop/server wall clocks. Server write authorization is always authoritative.

### 9.3 Status snapshot shape

Implement a typed, documented JSON DTO with at least this shape. Values below are illustrative, not current readings:

```json
{
  "schema_version": 1,
  "server_time": "2026-10-04T16:20:00.000Z",
  "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
  "state_version": 42,
  "server": {
    "version": "0.1.0",
    "environment": "development",
    "backend": "ble",
    "uptime_seconds": 120.0
  },
  "device": {
    "id": "s300",
    "name": "AP S300 V2.0",
    "address": "2A:02:01:48:6B:D0",
    "profile": "s300-v2-qualified-2026-10-04",
    "hardware_version": null,
    "firmware_version": null
  },
  "connection": {
    "phase": "connected",
    "reason_code": null,
    "message": "Connected.",
    "hints": [],
    "desired": "running",
    "link_connected": true,
    "session_id": "4d4a5cac-0304-44b0-b9ae-a48a4c4f5c30",
    "adapter_address": "F4:4E:FC:A1:CB:FF",
    "adapter_name": "Actions",
    "adapter_id": "hci2",
    "retry_in_seconds": null
  },
  "telemetry": {
    "state": "live",
    "age_seconds": 0.4,
    "sample": {
      "sequence": 90,
      "received_at": "2026-10-04T16:19:59.600Z",
      "segment_id": "3fb9c4d5-3a70-4cbb-b40e-01bc1e33b602",
      "battery_percent": 88,
      "input_power_w": 0,
      "output_power_w": 0,
      "remaining_minutes": 6795,
      "ac_enabled": false,
      "dc_enabled": false,
      "light_enabled": false,
      "status_flags": 12
    }
  },
  "controls": {
    "supported_outputs": ["ac", "dc", "light"],
    "allowed": true,
    "reason_code": null,
    "outputs_revision": 3,
    "pending_command_id": null
  },
  "history": {"enabled": true, "state": "ok", "interval_seconds": 10},
  "logging": {"state": "ok", "configured_level": "INFO", "effective_level": "INFO"},
  "diagnostics": {
    "valid_samples": 90,
    "rejected_frames": 0,
    "reconnect_attempts": 0,
    "history_dropped": 0,
    "log_records_dropped": 0,
    "last_scan_rssi_dbm": -70,
    "last_scan_rssi_received_at": "2026-10-04T16:18:01.000Z"
  }
}
```

Add nullable error summaries and nominal specifications without changing the above semantics. No raw frame appears in this normal DTO. `state_version` increases for published state changes and is distinct from `outputs_revision`. Counters are server-lifetime counters unless explicitly labeled otherwise. A new server instance invalidates previous sequence assumptions.

## 10. SQLite telemetry history

### 10.1 Sampling policy

**HIST-01:** Receive every valid BLE notification. Persist a periodic snapshot at `history.interval_seconds`, default 10 seconds. This setting controls database recording, not the station's notification rate.

Save the first valid sample of a new continuity segment promptly. Thereafter, at each monotonic recording deadline, save the newest eligible sample only if it is live, from the active session, and has a receive sequence newer than the last saved sample. Repeated equal *values from new notifications* are saved. Repeatedly stamping the same cached notification with new timestamps is forbidden.

Keep actual sample reception timestamps, not invented evenly spaced measurement times. Do not fill missed intervals with null, zero, duplicated, interpolated, or fabricated readings. No automatic aggregation, expiry, deletion, partitioning, or compaction job.

**HIST-02:** Assign a new continuity `segment_id` after restart, disconnect, intentional pause, stale/invalid telemetry interruption, recording interruption, or significant clock discontinuity. Later charts must not connect different segments. Empty periods do not produce telemetry rows. A segment identifier in telemetry is metadata, not a log table.

**HIST-03:** Raw BLE bytes never enter SQLite. Application logs never enter SQLite. A diagnostic eight-bit `status_flags` integer is allowed and is not the raw frame.

### 10.2 Initial schema

Use two tables and a schema version maintained with `PRAGMA user_version`. The logical schema below is normative; migration code must execute it transactionally. Require an SQLite runtime supporting STRICT tables, or report the missing prerequisite before storage use. STRICT tables are supported from SQLite 3.37.0 [E10].

```sql
CREATE TABLE devices (
    id INTEGER PRIMARY KEY,
    address TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    model TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE telemetry (
    id INTEGER PRIMARY KEY,
    device_id INTEGER NOT NULL REFERENCES devices(id),
    received_at_ms INTEGER NOT NULL,
    segment_id TEXT NOT NULL,
    battery_percent INTEGER NOT NULL CHECK (battery_percent BETWEEN 0 AND 100),
    input_power_w INTEGER NOT NULL CHECK (input_power_w BETWEEN 0 AND 65535),
    output_power_w INTEGER NOT NULL CHECK (output_power_w BETWEEN 0 AND 65535),
    remaining_minutes INTEGER NOT NULL CHECK (remaining_minutes BETWEEN 0 AND 65535),
    ac_enabled INTEGER NOT NULL CHECK (ac_enabled IN (0, 1)),
    dc_enabled INTEGER NOT NULL CHECK (dc_enabled IN (0, 1)),
    light_enabled INTEGER NOT NULL CHECK (light_enabled IN (0, 1)),
    status_flags INTEGER NOT NULL CHECK (status_flags BETWEEN 0 AND 127),
    CHECK (ac_enabled = ((status_flags & 2) != 0)),
    CHECK (dc_enabled = ((status_flags & 1) != 0)),
    CHECK (light_enabled = ((status_flags & 16) != 0))
) STRICT;

CREATE INDEX telemetry_device_time_id
    ON telemetry(device_id, received_at_ms, id);
```

The integer primary key prevents collisions from equal or backward-moving wall-clock timestamps. Device metadata does not implement multi-device acquisition; one configured active station remains the v1 scope. Rows from an earlier configured identity must not be silently relabeled.

### 10.3 Access and durability

Use explicit parameterized SQL through `aiosqlite`, with short transactions and one owned connection/transaction lock. Bound pending storage work and concurrent query admission. Default rollback journal mode is **DELETE**, with `synchronous=FULL`, `foreign_keys=ON`, and a 5,000-ms busy timeout. This small periodic single-writer workload does not require WAL or a separate checkpoint subsystem. Verify the actual PRAGMA results and runtime SQLite version. [E6, E11]

Do not call synchronous SQLite/file work on the BLE event loop. Do not enable `synchronous=OFF`. Never create a fresh empty database over a corrupt or unknown-newer schema. An incompatible schema is a visible storage error requiring operator action; reads/diagnostics and live acquisition must remain available where possible.

Storage failure must not hide live telemetry or kill the daemon merely because a directory is read-only, the disk is full, or recording temporarily fails. Mark history degraded, record dropped samples, use bounded retry/reopen behavior, and begin a new continuity segment after recovery. Do not accumulate unlimited samples in RAM. Telemetry controls still obey their own protocol/auth checks, not database availability.

Default maximum queued recording jobs: 256. Do not evict arbitrary rows to recover space. Report the database path, last successful write, pending count, and storage error in authenticated diagnostics.

### 10.4 History queries

Support UTC `[since, until)` ranges, ordered by `(received_at_ms, id)`, with default limit 1,000 and maximum 10,000. Require bounded pages. A first request fixes a high-water row ID; a cursor includes that boundary and the last ordered key so later inserts do not make pagination chase a moving target. Validate cursor size/types and filter identity; bind all SQL parameters. Reject invalid/expired database-instance cursors clearly.

Return actual samples with segment IDs and timestamp semantics, not aggregates. A range with no data returns an empty `items` array. A disabled/degraded history store is distinguishable from a successful empty query. Long queries have a bounded deadline and must not block Bluetooth or request handling.

### 10.5 Backup boundary

Backup is the user's Proxmox/LXC responsibility. Document the configuration, data, log, and Caddy-state paths that may need inclusion; do not implement a backup service. A live database backup requires a consistent snapshot or SQLite-aware backup, not an arbitrary copy of only an open database file. Restore validation must open the restored copy separately and run an integrity check. Never assume an external mount is included just because the LXC root filesystem is backed up. [E11, E12]

## 11. Structured logs and remote diagnostics

### 11.1 Storage and representation

The server writes application-owned JSONL: exactly one UTF-8 JSON object per physical line. Default production path: `/var/log/mypowers/mypowers.jsonl`. Use size rotation, 10 MiB per file and five backups by default. This log rotation is independent of telemetry-history retention, which remains disabled.

A record contains `schema_version`, UTC `timestamp`, `server_instance_id`, process-local sequence, `level`, `logger`, stable `event`, readable `message`, and a bounded `context` object. Include request/command ID and connection stage where relevant. Escape newlines inside exception strings so one record remains one line.

```json
{"schema_version":1,"timestamp":"2026-10-04T16:20:00.100Z","server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10","sequence":12,"level":"INFO","logger":"mypowers.bluetooth","event":"ble_connected","message":"Bluetooth connection established.","context":{"device_id":"s300","adapter_id":"hci2"}}
```

JSONL is the persistence format, not the required human display. The CLI renders timestamps, levels, and messages through Rich; the native TUI uses Ratatui. Explicit `--json` returns structured data.

### 11.2 Logging behavior

Use standard logging with structured fields and a bounded queue/listener so file writes/rotation do not block notification processing [E13]. Default queue capacity: 2,048 records. Filter before enqueuing, prioritize operational records over DEBUG traffic, expose drop counters, and report overload without recursively logging the overload into the same full queue. Flushed log records should normally be available to queries within one second.

INFO records connection/health transitions, command outcomes, startup/shutdown, configuration provenance without secrets, and recording failures. Do not log every normal telemetry sample at INFO. Rate-limit identical repeated errors and emit summaries rather than growing logs with repeated stack traces.

DEBUG may include complete received/transmitted BLE frames, validation reasons, and detailed transport stages. Only DEBUG may contain `frame_hex`/equivalent raw bytes. Limit record size, default 16 KiB; explicitly mark truncation. Preserve enough raw data for the verified 16-byte status and 9-byte control frames. Do not claim that application logs include privileged HCI captures.

Redact API tokens, authorization headers, cookie values, dotenv contents, DNS credentials, and secret-bearing URLs before every sink, including exceptions and access logs. Device names and error strings must not inject Rich markup or terminal escape sequences. Non-color JSON output must not contain terminal controls.

Mirror readable development logs or JSONL service logs to stderr when configured. Journald/OpenRC may capture them, but remote application-log access must not depend on either supervisor. Do not query system-wide logs or execute `journalctl` on behalf of an API caller.

### 11.3 Remote reads and follow

Provide authenticated log tail, UTC date-range filtering, minimum-level filtering, and follow. Default CLI tail: 10 records; API default page: 100, maximum: 1,000. Queries cover only this application's active and retained rotated files. No path parameter, glob, SQL, or shell command is accepted from a client.

File queries run off the event loop with bounded memory/time. Handle a partially written final line and malformed/truncated old lines without crashing; skip with a diagnostic count. Rotation during a query/follow must neither crash nor silently duplicate records. Use opaque cursors over known file identities/offsets, never expose unrestricted filesystem paths as a query facility. Return `410 log_cursor_expired` when rotation removes a cursor's source.

A query that cannot complete within its scan budget returns a resumable partial page or an explicit bounded-query error; never silently imply that the full time range contained no other records. No search index or log database is required. A follow stream begins after a specified retained cursor and emits a gap notice or an explicit expiry when it cannot bridge the interval. It must remain bounded when a client stops reading.

### 11.4 Runtime log level

Implement authenticated runtime changes to `DEBUG`, `INFO`, `WARNING`, or `ERROR`. `mypowers debug on` selects DEBUG until restart; optional `--duration` selects a bounded duration, after which the baseline is restored. `mypowers debug off` removes the runtime override and restores the resolved startup baseline, which is not necessarily INFO.

Expose configured/effective level and override expiry. Do not modify `.env` or YAML on disk remotely. Record the change, suppress secrets, and keep transport-level DEBUG activation scoped to relevant application/Bleak namespaces rather than enabling verbose output for every installed library.

A file-sink failure marks logging degraded; keep a bounded in-memory recent-log ring and stderr fallback. API responses must identify the available source and lost history rather than claiming durable logs were retained.

## 12. HTTP and WebSocket contract

### 12.1 General rules

Use `/api/v1`. HTTP is JSON except streaming WebSockets. Generate OpenAPI from typed request/response models; document stream schemas separately and check example messages with tests. Reject unknown mutation fields, string substitutes for booleans, oversized bodies, invalid enum values, invalid ranges, and timezone-naive API datetimes.

All state-changing, history, status, log, and stream endpoints require the API token in production. A minimal process-health endpoint may be unauthenticated and must reveal no station identity, credentials, filesystem paths, or telemetry. `Cache-Control: no-store` applies to API responses containing state/diagnostics. Do not follow arbitrary redirect destinations while sending credentials. Clients accept only HTTP(S) base URLs without userinfo, query, or fragment, derive WS/WSS from the same origin, verify TLS certificates/hostnames, and disable ambient HTTP proxy discovery by default for this local-service client. An explicit CA-file option may add trust for testing; no insecure verification bypass is provided.

Use a consistent error envelope:

```json
{
  "schema_version": 1,
  "error": {
    "code": "state_conflict",
    "message": "Station output state changed; refresh before retrying.",
    "retryable": false,
    "request_id": "fa43e43c-8bb1-4c06-956b-6aa2d9a2f222"
  }
}
```

`retryable` describes whether a later fresh operation may be sensible. It never instructs an HTTP client to automatically replay a physical-control mutation. Validation errors must not echo tokens or unsafe raw request content.

### 12.2 Endpoints

| Method/path | Request | Required response/behavior |
|---|---|---|
| `GET /health/live` | None | 200 when the HTTP process responds; minimal `{"status":"ok"}` |
| `GET /api/v1/status` | None | Full snapshot from Section 9; 200 even when S300 is missing |
| `GET /api/v1/capabilities` | None | Read metrics, supported outputs `ac/dc/light`, qualified profile/limits; no invented capabilities |
| `GET /api/v1/history` | `since`, `until`, `limit`, optional `cursor` | Raw sample page and next cursor; empty array if successfully queried range is empty |
| `PUT /api/v1/outputs/{output}` | Desired state body below; `Idempotency-Key` | 202 admitted operation; typed precondition errors otherwise |
| `GET /api/v1/commands/{command_id}` | None | Latest retained operation/result; 404 if unknown/expired |
| `PUT /api/v1/connection` | `{"desired":"paused"}` or `{"desired":"running"}` | Runtime intent/status; no station control frame |
| `POST /api/v1/connection/retry` | Empty object | Schedule/coalesce recovery wakeup; do not overlap active work |
| `GET /api/v1/logs` | `tail` or `since/until`, `min_level`, `limit`, `cursor` | Structured application-log page, retention/gap metadata |
| `PUT /api/v1/runtime/log-level` | `level`, optional `duration_seconds` | Baseline/effective override information |
| `DELETE /api/v1/runtime/log-level` | None | Remove override; restore baseline |
| `WS /api/v1/events` | Stream authentication | Full initial snapshot, state/command events, heartbeats |
| `WS /api/v1/logs/stream` | Stream authentication and optional log cursor/filter | Structured log events, heartbeats, explicit gap/expiry behavior |

No raw-frame, remote shell, file editor, generic GATT write, firmware/settings, or service-restart endpoint.

History/log query timestamps are UTC-aware instants; the server normalizes offsets to UTC. `until` defaults to server time and `since` to one hour before it for history. Log `tail` and time-range mode are mutually exclusive. Limits and cursors are exposed through CLI flags; no hidden unbounded pagination.

### 12.3 Output requests

`{output}` is exactly `ac`, `dc`, or `light`:

```json
{
  "enabled": true,
  "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
  "expected_outputs_revision": 3
}
```

Admitted response:

```json
{
  "schema_version": 1,
  "command_id": "792057c3-d840-47ea-9f2f-54e7e2d9ab24",
  "status": "accepted",
  "output": "light",
  "requested_enabled": true,
  "created_at": "2026-10-04T16:20:01.000Z",
  "completed_at": null,
  "reason_code": null,
  "observed_flags": null
}
```

The command resource later includes `confirmed_at`, confirmation sample sequences, outcome reason, and last observed flags as applicable. Do not put raw bytes in ordinary command responses. A command ID is an application identifier, not a device protocol transaction ID.

Use 401 for missing/invalid credentials, 403 for an explicit policy prohibition, 409 for busy/conflict/unqualified-current-profile, 422 for invalid request data, 413 for oversized bodies, 429 for admission/rate limits, and 503 for unavailable telemetry/infrastructure/storage needed by that operation. Once admitted, terminal command outcomes live in the command resource, not a delayed unrelated HTTP error.

### 12.4 Authentication and browser readiness

Native clients send `Authorization: Bearer ...` for HTTP and may do so during WS handshake. Never accept long-lived credentials in a URL/query parameter. Use constant-time secret comparison. Token file permissions and secret redaction are tested.

For a later browser, support WS authentication as the first JSON message after upgrade: `{"type":"authenticate","token":"..."}`. Send no application data until successful authentication; close unauthenticated connections after five seconds. Cap message size and unauthenticated connection count. Header-authenticated clients do not need to repeat this message.

Validate any browser `Origin` against the configured public origin or explicit loopback development origins before accepting a stream. Native clients without `Origin` still require their token. Do not enable permissive wildcard CORS. Browser cookie login/session storage is outside scope; accepting first-message WS authentication does not implement a web login interface. [E2]

All control requests remain HTTP operations; WebSocket streams are observational in v1. Auth, rate/body limits, and origin checks apply independently of Caddy TLS. Require JSON Content-Type for JSON mutation bodies. Enforce an allowed Host set consisting of loopback names/addresses and the configured production hostname; do not accept arbitrary DNS-rebinding Host values. Reject a supplied untrusted Origin on mutation requests as well as streams.

### 12.5 Stream behavior

Each authenticated event stream receives a full status snapshot first. Thereafter send typed `state`, `command`, and `heartbeat` messages, with `schema_version`, `server_instance_id`, a connection-local monotonically increasing `stream_sequence`, and `server_time`. State messages contain the complete latest snapshot, avoiding fragile client-side patch reconstruction.

A heartbeat is sent at least every five seconds, including during disconnection from the station. Clients mark the server connection lost after 15 seconds without a valid message/heartbeat, even if the socket has not yet closed. Distinguish server loss from a reachable server reporting S300 loss.

Bound authenticated WS clients to 16 by default. Bound each state stream's pending queue to 128 messages and each log stream's to 256. Coalescing superseded state snapshots is permitted; do not silently discard command outcomes. When a slow client cannot be kept current, close it with an explicit retryable stream reason (e.g. 1013), then let it reconnect and obtain a new snapshot/query outstanding command IDs. Never await a slow client's send from the BLE callback.

Application-state streams do not promise durable event replay. After reconnect/server restart, discard old transport sequences and resynchronize from the initial full snapshot. Log streams use their explicitly bounded file/ring cursor contract. Duplicate/malformed messages must not trigger client commands.

### 12.6 Request/resource limits

Default HTTP body limit: 16 KiB, including chunked requests. Bound query time, accepted WebSocket messages, command results, and subscription counts. Allow enough capacity for CLI + TUI + later clients without enabling infinite work. Local command admission (one operation at a time) is mandatory even for authenticated users. Heavy history/log requests must run off-loop and have bounded concurrency, separate from BLE ownership.

Do not build business logic around OpenAPI defaults alone. Test request validation, exception mapping, response schema, authentication on every applicable route, and shutdown while requests/streams are active.

## 13. CLI requirements

### 13.1 Behavior and presentation

The CLI is a short-lived API client. Normal commands send requests, print their result, and exit. They do not start the daemon, connect to Bluetooth, query local SQLite, take ownership of the adapter, or create a full-screen terminal UI.

Use Rich for minimal human-readable output: readable labels, restrained colors, units, and optionally a static battery bar or compact borderless tabular output for multiple history/log rows. Default status is a short label/value summary, not a large boxed dashboard. Do not use `Live` or an alternate screen for ordinary CLI commands. A control command may wait for confirmation without continuously redrawing the terminal.

`--json` bypasses human rendering entirely: one complete JSON response/error object on stdout, no ANSI, no banner, no debug output. Diagnostics go to stderr. For explicitly streaming `logs --follow --json`, use one JSON object per line and document this exception. Handle broken output pipes without a traceback.

### 13.2 Required commands

```text
mypowers --help
mypowers --version
mypowers --env-file .env status
mypowers --server https://mypower.efez.net status
mypowers status --json
mypowers status --require-live
mypowers capabilities --json
mypowers ac on
mypowers ac off
mypowers dc on
mypowers dc off
mypowers light on
mypowers light off
mypowers command COMMAND_ID --json
mypowers history --since 1h --limit 1000
mypowers history --since 2026-10-04T12:00:00Z --until 2026-10-04T13:00:00Z --json
mypowers logs --tail 10
mypowers logs --since 30m --level WARNING
mypowers logs --follow
mypowers logs --follow --json
mypowers debug on
mypowers debug on --duration 15m
mypowers debug off
mypowers connection pause
mypowers connection resume
mypowers connection retry
mypowers tui
```

Support `--env-file`, `--server`, `--token-file`, `--ca-file`, request timeout, `--no-color`, `--timezone`, `--utc`, and `--json` where appropriate. Avoid a token-as-command-argument flag that leaks through process listings/history. Provide consistent option placement in the parser and tests; the examples above must work.

Relative `--since` ranges use the server's reported current time where practical. Explicit local dates/times require an identified timezone; API requests always carry aware timestamps. `--utc` overrides presentation timezone. Logs display the timezone/date when omitting them would be ambiguous.

Control commands fetch a current status, submit one explicit intention with its output revision/server instance and a new idempotency key, then wait for that command resource to become terminal. Default client confirmation wait budget: 25 seconds, covering the bounded server operation. Do not repeatedly issue the PUT while waiting. On ambiguous timeout, print the command ID and last known result, exit nonzero, and never claim the output remained unchanged.

Log `--follow` is the deliberate streaming exception to one-shot behavior; it does not use a full-screen UI. Closing it must not affect the server's logging or BLE session.

### 13.3 Exit codes

| Code | Meaning |
|---|---|
| 0 | Request completed; control is `confirmed`/`no_change`, or status was successfully retrieved |
| 1 | Other application/query/internal error |
| 2 | CLI usage or local configuration error |
| 3 | Cannot reach server, transport/TLS failure, or incompatible API response |
| 4 | Authentication/authorization failure |
| 5 | Device/control precondition failure, state conflict, busy, or `--require-live` not met |
| 6 | Control outcome unknown/unconfirmed or confirmation wait expired |
| 130 | User interruption |

A successful `status` request returns 0 even if its body says the station is disconnected; `--require-live` provides script-friendly stricter behavior. Human-readable and JSON modes must use the same exit-code semantics.

## 14. Ratatui TUI requirements

### 14.1 Full application, not a repeating CLI print

The TUI is a persistent, remote-capable terminal application with live state, keyboard focus, mouse clicks, control outcomes, power trends, and logs. Implement with Rust, Ratatui, and Crossterm using the approved standalone prototype as the visual reference.

Ratatui is the renderer; use its Terminal, Frame, Layout, Block, Paragraph, Sparkline and Buffer primitives with Crossterm input. Use an explicitly owned rendering loop at no more than four frames per second. Avoid an additional auto-refresh thread racing with application state/input updates.

### 14.2 Visual design

Cap the bordered dashboard at 94 columns by 28 rows, with a separate borderless
status row below it (94×29 total). Center the composition in larger terminals.

```text
+----------------------------- MYPOWERS --------------------------------+
| AP S300 V2.0                 SERVER CONNECTED           TELEMETRY LIVE |
|                                                                        |
| BATTERY                         88%                                    |
| [============================........]                                 |
|                                                                        |
| INPUT                            OUTPUT                                |
| 55 W                             25 W                                  |
| recent 120-second trend          recent 120-second trend                |
|                                                                        |
| AC  [ OFF ]          DC  [ OFF ]          LAMPS  [ OFF ]                 |
|                                                                        |
| Station estimate  6h 40m                                               |
|                                                                        |
+---------- a AC  d DC  l lamps  F3 logs  ? help  q quit -----------------+
 Lamps ON confirmed
```

This is a layout guide with illustrative values, not a requirement to copy ASCII borders or display those readings. Use Ratatui widgets and layout constraints, consistent spacing, and an uncluttered palette. Battery is the strongest visual value; input and output have equal weight. State is expressed by text as well as color. Honor `NO_COLOR`/terminal capabilities. No emoji or Nerd Font dependency.

Retain the approved prototype layout with inline INPUT/OUTPUT readings above two-row graphs. At 60×19 retain primary data, controls, and the status row. Below 60×19 show a readable resize message with a working exit key. Resizing must not cause exceptions, stale hitboxes, or accidental commands.

Keep hotkeys in the dashboard's bottom border. The status row contains one concise
recent action or meaningful connection transition, with no command UUIDs, raw
flags, internal reason codes, or stack traces. Live operational INFO/WARNING/ERROR
messages can provide feedback; DEBUG and historical replay cannot. Fade the
foreground/style in four two-second stages, then clear after eight seconds. A new
message replaces the previous one. Repeated telemetry must not restart the timer.
Use green for success, muted cyan for information, yellow for warnings, and red
for errors. Reserve an independent right-aligned region for future persistent
alerts. While Logs is open, show `Log: LEVEL` there and the selected-timezone
override expiry only when a future expiry exists. On the normal dashboard, keep
the right region empty until alerts exist; never render zero counters or
reassuring health text. Preserve the right region and one separating space when
shortening a long left message with an ellipsis. Keep full details in Logs/API.
Permanent age, history health, adapter, and log-level diagnostics belong in the
Settings diagnostics view, not on the normal dashboard. The current read-only
Settings overlay shows preferences and runtime diagnostics; mutable server
settings remain a separate feature.

### 14.3 Data and controls

Connect to the event stream and render actual DTOs. The initial screen can exist before the API or station is available. Keep server connectivity separate from station telemetry. A stale or disconnected sample is visibly last-known; unknown values are dashes/Unknown, not zero/Off.

Each AC/DC/lamp control supports mouse activation and keyboard focus/activation. Show an explicit pending target while the command is active; keep the observed switch state separate. Disable all further output mutations while the server is busy or local/server freshness is unsuitable. Server revalidation is required even when the UI enabled a button.

On denied/conflicting/unconfirmed commands, show a concise message and leave observed state sourced from telemetry. Do not optimistically flip a switch, replay failed clicks, or re-send a desired state after reconnect. State changes caused outside MyPowers must appear without a local command.

### 14.4 Trends and logs

Keep at most 120 seconds of received power samples and a hard cap of 512 live samples in the client. Optionally initialize from the same API's persisted last-120-second history; preserve the actual, potentially sparser timestamps. Do not make the client open SQLite.

Use small input/output sparklines or equivalent terminal trends. Render using actual timestamps. Different `segment_id` values and missing intervals produce visible gaps; do not bridge outages. Grouping real samples into terminal columns is display reduction, not stored aggregation. Do not infer Wh, charging state, or solar irradiance from these plots.

For each power graph independently, show a dim eleven-cell idle track with a
small circle moving one cell every two seconds and reversing at the ends only
when fresh live power is zero and no positive sample remains in its 120-second
history. Preserve this delay even when zero overwrites a positive sample in the
same display column. Positive current power immediately restores the normal
graph; stale, unknown, or disconnected telemetry must not animate the idle track.
Use Ratatui widgets within the existing render loop.

Provide a Logs overlay modal with day navigation, level filtering, vertical scrolling and a draggable scrollbar. Its two-row header contains day navigation followed by mode, timezone, `Filter ≥ LEVEL`, and page size. Put log navigation/filter/page-size/DEBUG shortcuts in the modal bottom border and close/help/quit shortcuts in the outer bottom border. Runtime log level and any future override expiry appear on the status strip's right side only while Logs is open. No refresh button or shortcut is needed: show the new-record count, use Home for the selected day's beginning, and End to fetch today/live bottom.

Translate local calendar day boundaries in the selected IANA timezone to UTC `since`/`until` filters. Load older/newer pages lazily through the authenticated API; never read log files in the frontend. Keep at most five loaded archive pages and 1,000 recent stream records. The session page-size control supports 50, 100, 250, 500 and 1,000 records per request; persistence through mutable server settings is a separate task. Archive mode stays stationary while new records arrive. Scrolling beyond a completed day's start/end enters the previous/next day. Today's completed bottom enables live follow; scrolling away disables it. These actions use the documented authenticated API. Application errors must not overwrite the alternate screen with uncontrolled tracebacks.

### 14.5 Input and terminal restoration

Implement at least Tab/Shift-Tab focus, Enter/Space activation, keyboard view selection, scrolling where relevant and help. Esc closes a modal and returns to the dashboard; it never exits. `q` opens quit confirmation; Ctrl+Q exits immediately. Mouse buttons and scroll must work in a Linux terminal supporting SGR mouse reporting. Provide `--no-mouse` for keyboard-only use.

Only q, Ctrl+Q, and Esc are global. Other keys belong to the active context and
must not fall through from modals to dashboard actions. F3 opens Logs from the
dashboard; s opens read-only Settings / Diagnostics. F1/? open Help over the
dashboard for the active dashboard, Logs, or Settings context. Dashboard
r/p retry or pause acquisition; b toggles DEBUG in Logs or Settings. Help content and
border hotkeys match the active context. Remove F2 and log refresh actions.

Keep terminal event decoding isolated and tested. Use bounded incremental parsing for fragmented escape sequences. Recognize SGR mouse sequences, resize, and bracketed paste; ignore pasted content as control actions. Register an action once per complete click, not once for both press and release. After layout/state changes, do not apply an old press/release pair to a different widget. Prevent duplicate mutation from double clicks while a command is pending.

Record original termios/terminal settings before modification. Use cbreak/noncanonical input as appropriate while retaining signal handling. Enable only needed mouse modes (e.g. button reporting and SGR coordinates), not unrestricted mouse-movement floods. Restore all modes changed by the program: mouse reporting, bracketed paste, cursor, alternate buffer, and original termios. Use `try/finally` and signal-aware shutdown. [E15, E16]

Test normal exit, Ctrl+Q, external SIGINT/SIGTERM, initialization failure, API failure, render exception, and resize. Typed Ctrl+C/Ctrl+Z are ignored in raw mode. No cleanup guarantee is possible after SIGKILL or terminal destruction; do not claim one. After supported exit paths, the shell must accept input with normal echo and no mouse escape garbage, without requiring `reset` or `stty sane`.

In non-TTY output/input, refuse full-screen mode with a useful message directing the user to the CLI. A TUI connection loss never stops server collection.

### 14.6 Deterministic UI snapshots

`cargo xtask ui-snapshots` generates SVGs in `artifacts/ui/` using fixed fake
state, a frozen rendering clock, and Ratatui TestBackend. Call the same production
`ui::draw` used by the interactive TUI and export its Buffer cells; never duplicate
the UI layout in HTML or a second renderer. No daemon, BLE, HTTP, or terminal is
required. Preserve cell positions, Unicode, foreground/background colors, bold,
and dim. Cover live, idle, reconnecting, device/daemon offline, pending commands,
Logs/Settings/Help overlays, resizing, and the undersized-terminal message.
Report success only after every requested artifact has been written; failures
return nonzero. SVG is canonical; PNG conversion is optional for inspection.

## 15. Deployment and security

### 15.1 Two deployment modes

Development:

```text
CLI / TUI -> http://127.0.0.1:8765 -> mypowersd -> local BlueZ -> S300
```

Production:

```text
LAN clients -> https://mypower.efez.net:443
                          |
                        Caddy
                          |
                  http://127.0.0.1:8765
                          |
                       mypowersd
                          |
                 BlueZ / Bluetooth -> S300
```

Caddy and the backend are in the same LXC for the reference production topology. Public WS URLs use WSS through the same proxy. Rich is not required in that server-only installation. HomeStack, DNS configuration, LXC sizing, USB/device exposure, OS selection, backups, and installation of system packages remain operator responsibilities.

### 15.2 Required environment, not automatic OS installation

Document a supported Python interpreter, Python's SQLite support, Linux Bluetooth kernel/driver support, an accessible system D-Bus, BlueZ exposing the adapter, and permissions for the chosen process account. `uv` is the build/install tool; Caddy is needed only for the production TLS edge. `bluetoothctl`, `rfkill`, `lsusb`, and `sqlite3` command-line tools are optional operator diagnostics, not subprocess protocols used by the application.

Alpine versus Debian is a deployment decision. Do not hard-code `apt`, `apk`, systemd, or `/home/user/DEV/tmp` in runtime logic. OS/runtime dependency checks should report missing capabilities clearly. Do not create a privileged LXC, bypass container restrictions, or grant capabilities automatically.

The reviewed research proves the QEMU route, not LXC hardware availability. An LXC deployment check must verify that its own BlueZ exposes the intended MAC and that the same qualified device can be acquired. Do not treat copying the Python code as proof of Bluetooth passthrough.

### 15.3 Production package installation

Deliver a normal wheel and reproducible locked server/client dependency exports, with tested install commands. The production supervisor invokes an installed `mypowersd`, not `uv run` in a development checkout that might create a new venv implicitly.

Respect the operator's production preference for a single-purpose container without an application-created venv. `uv pip install --system` is a deployment option for an appropriately provisioned interpreter, not permission to bypass an OS's externally-managed-environment protections. Do not automatically pass `--break-system-packages`; document the external prerequisite when the selected distribution prohibits system modification. [E4]

Provide supervisor examples named `mypowers.service` and `mypowers` for systemd/OpenRC respectively. They run the same foreground executable, select the env file explicitly, restart on genuine process failure, and permit orderly SIGTERM cleanup. Device absence is not a process failure. Include conservative restart delay and writable state/log/runtime directories. A dedicated service account is a documented option; account provisioning is external. Do not enable device-isolation hardening switches that break the actual tested BlueZ access without validating them.

### 15.4 Caddy and DNS-01

Production hostname is exactly `mypower.efez.net`, not `mypowers.efez.net` and not an added `home` subdomain. Local DNS resolves it to the LXC's private LAN address.

Obtain a public-trust certificate with ACME DNS-01. Public DNS must allow creation/removal of `_acme-challenge.mypower.efez.net` TXT records; the web service does not need a public A/AAAA record or inbound Internet reachability. DNS-01 does not require router port forwarding. Caddy needs outbound connectivity for DNS/API/ACME and a writable persistent certificate store. [E17, E18]

Caddy handles issuance/renewal and WebSocket reverse proxying. The Python process does not generate TLS certificates or implement renewal schedules. Use modern Caddy defaults; do not hard-code a certificate lifetime. Keep its administrative interface local and its DNS credential outside the application config/repository. [E17, E19]

**External input not supplied:** the DNS provider/API/module and its credentials. Do not invent a provider or claim a working public certificate without them. Deliver `Caddyfile.production.example` with a clearly identified provider-specific TLS import/snippet requirement, reverse proxy to `127.0.0.1:8765`, and instructions for installing the matching DNS module. No silent fallback to a publicly inaccessible HTTP-01 challenge or to untrusted internal TLS in production. Caddy DNS providers may require a module not present in a standard binary. [E20]

Deliver a separate local TLS test configuration using an isolated Caddy internal CA, loopback/unprivileged test port, and disabled automatic system-trust installation. Verify HTTPS and WSS using that explicit test CA without `verify=False`. This proves proxy/TLS integration, not production DNS-01 or phone trust. Do not install a CA into the user's system trust store without permission.

### 15.5 Security invariants

- Authentication protects status/history/logs/control independently of unauthenticated BLE hardware. The user API must not add an open network control path.
- Backend HTTP is loopback-only in production; LAN exposure goes through Caddy. Do not provide an insecure fallback or auto-disable auth because TLS terminates upstream.
- Honor forwarded headers only from the configured loopback proxy. Do not trust arbitrary forwarded client identity/scheme for authorization.
- Do not accept tokens in URLs, log credentials, serve `.env`/research files, expose arbitrary filesystem paths, or execute remotely supplied commands.
- Keep browser origin/CORS policies narrow. Disable interactive API documentation in production by default or protect it explicitly; retain a generated OpenAPI artifact for development.
- Keep expected hardware identity/profile and the command allowlist at the BLE transport boundary, even when requests originate from an authenticated frontend.
- Do not auto-write on boot/reconnect, restore stale output snapshots, or override physical changes. Loss of telemetry is a control denial, not permission to use cached state.

## 16. Implementation sequence and validation

### 16.1 General execution rules

The coding agent must implement, execute tests, inspect actual results, fix problems, and continue through all milestones. Maintain a small implementation checklist with requirement IDs. Do not replace executable tests with prose stating that something “should work.” Do not stop when only the server is complete.

Inspect the current checkout and `git status` before editing. The reviewed starting revision is `52dc203...`; if the repository advanced, inspect those changes and preserve them rather than resetting it. Respect existing repository instructions and license. Do not overwrite unrelated user work, delete research, force-push, or push remotely unless separately authorized. Make focused local commits of implementation/tests/docs; report their hashes.

Do not require broad new protocol research or subagent orchestration. Never run two hardware-owning processes in parallel, including an old demo, a test runner, a dev reloader, and the daemon. Tests using a fake transport may run independently.

### 16.2 Milestone A — Reproducible foundation and evidence baseline

1. Read R1–R6 in the specified order, prioritizing the current matrix over old restrictions.
2. Re-run the imported offline baseline from its actual directory:

```bash
cd docs/research/s300
uv run --no-project --with bleak==3.0.2 --with dbus-fast==3.1.2 \
  python -m unittest test_control_allpowers.py test_s300_matrix.py test_s300_connection_lab.py
```

3. Record the real count/result, Python/Bleak/dbus-fast versions, and starting Git revision. Do not change research to make the baseline green.
4. Create the package/extras/build configuration, `uv.lock`, formatting/lint/type checks, and test configuration. Add a minimal root README and ignore rules.
5. Verify that no normal import/test collection contacts hardware. Establish a fake BLE port, injected clock, and isolated filesystem/config fixtures.

### 16.3 Milestone B — Protocol, state, and BLE ownership

Implement and test pure protocol functions first, using captured vectors and independently recomputed checksums. Implement the independent connection/freshness states, selected-adapter resolver, a single owned BLE session, cancellation/cleanup, and bounded recovery.

Provide an explicitly simulated backend for development/integration tests that does not import or contact Bleak. It must visibly identify itself as simulated, default to separate temporary data, and be refused in production. Do not silently switch to it when real Bluetooth fails.

Perform a short real, passive connection check using the application's transport once offline tests pass. A historical demo may be used only as a separate bounded diagnostic step with clean release before starting the application. Record live current values, not hard-coded historical readings.

### 16.4 Milestone C — Service, API, storage, diagnostics, and control

Implement application-owned command transactions, registry/deduplication, HTTP/WS auth and DTOs, SQLite history, JSONL logs/rotation, remote log access, and runtime debug changes.

Keep the API responsive during connection attempts and storage/log work. Exercise the full system against a fake transport before real writes. The initial CLI may be developed here as a thin test client; it must call the API, not bypass the core.

Milestone exit requires a working remote status request, saved real telemetry, a state stream, an authenticated log tail, and verified control behavior in offline integration tests. Then run authorized real output acceptance through the API.

### 16.5 Milestone D — Complete CLI and Ratatui TUI

Implement every required CLI command/mode and exit code. Implement the full interactive TUI, including mouse/keyboard operation, update streams, pending/outcome rendering, log view, and terminal restoration. The Python CLI and native Rust TUI implement the same documented HTTP/WS contracts; test both against the daemon.

Test the UI with a fake API and a pseudoterminal before using real controls. Then connect CLI and TUI to the same real daemon and confirm that frontend lifecycle does not create extra BLE connections or stop collection.

### 16.6 Milestone E — Packaging, deployment examples, and handover

Build/test installable artifacts, CI, env/YAML examples, systemd/OpenRC examples, local Caddy TLS integration, and operations documentation. Run the final offline suite and hardware acceptance/soak. Resolve failures and generate the validation report, API schema, and requirement checklist.

Production DNS issuance/LXC-specific deployment can be marked externally blocked only when the missing provider/credential/environment genuinely is not available. Do not call a mock proxy test a production deployment. Application implementation must still be completed and locally validated.

### 16.7 Mandatory offline test inventory

| Area | Required cases |
|---|---|
| Protocol | All eight exact TX frames/checksums; captured status fixtures; all 24 directed single-switch encodings; no-op requests; preservation of unrelated/persistent bits; RX/TX light-mask distinction |
| Invalid data | Invalid header/envelope/length/command/checksum; battery >100; zero and uint16 boundaries; unobserved high flags; unsupported lower profile; truncated/concatenated data; no speculative parser repair |
| Freshness | First-sample deadline; stale threshold; independent connected/live observations; invalid frames do not reset freshness; monotonic behavior under wall-clock jumps; old-session callbacks ignored |
| Infrastructure | Missing/powered-off/blocked/not-ready adapter; unavailable bus vs unavailable BlueZ vs denied access; stale BLEDevice; service/adapter restoration; no fallback; no mutation of real host services |
| Connection ownership | Serial discovery/connect/cleanup; explicit retry coalescing; pause/resume; failed cleanup prevents new client; duplicate process lock; shutdown during every phase |
| Commands | Newly received pre-write sample; full lock through confirmation; busy/conflict handling; same-key deduplication; conflicting key body; no-change without write; same-instance/revision guards; client disconnect after admission |
| Confirmation | Two consecutive post-write complete states; dynamic numeric fields allowed; one match insufficient; stale/pre-write/old-session/invalid/unrelated frames rejected; delayed response cannot confirm another command |
| Failure outcomes | Transport timeout after potential send; lost notification/link; unconfirmed status; no replay/retry; resynchronization before further writes; no automatic output enforcement or shutdown reset |
| SQLite | Schema constraints/migration; future schema refusal; timestamp collisions/backward jumps; same values from new samples saved; same cached sample not re-stamped; zero vs absent; gaps/segments; recording interval; no raw-frame/log columns |
| Storage resilience | Full/read-only/unavailable/corrupt database; queue bounds; recovery without daemon death; bounded queries; stable pagination under insertion; disabled history vs empty range |
| Logs | Valid one-object-per-line JSONL; redaction at all sinks; raw frames only at DEBUG; level/expiry/reset; rotation/query/follow races; malformed final lines; cursor expiry; slow consumers; file failure/ring fallback |
| API | Every auth boundary; strict booleans/enums; invalid/oversized bodies; error envelope; no raw-control path; process health independent of BLE; operation IDs and OpenAPI response consistency |
| WebSockets | Header/first-message auth; unauthorized timeout; rejected Origin; snapshot before updates; heartbeat; reconnect/resync; slow readers; bounded clients/queues; no secret echo |
| CLI | Each command; human vs JSON; no ANSI/logs on JSON stdout; stable exit codes; unreachable server vs disconnected station; command uncertainty; broken pipe; explicit log follow |
| TUI | Mouse and keyboard hitboxes; fragmented input; press/release deduplication; paste ignored; stale/pending controls; resize; no-color; API loss; exit/error/signal terminal restoration |
| Packaging | Wheel installs outside checkout; server-only without Rich; client-only without BLE/server/storage libraries; missing-extra hints; no runtime dependency on research paths |
| Configuration | CLI/env/dotenv/YAML precedence; relative path sources; missing/duplicate/unknown keys; secrets redacted on errors; production safety; no implicit simulated fallback |

Use pytest/pytest-asyncio and deterministic fakes for production tests. The imported research unittest suite is a separate baseline. Test APIs both in-process and through a real local Uvicorn subprocess with the simulated backend. No offline/CI test may discover or control the user's physical station.

### 16.8 Terminal validation

Use Linux pseudoterminals to spawn the installed CLI/TUI against a fake API server. Inject keyboard and SGR mouse sequences, terminal-size changes, malformed/fragmented input, and termination signals. Compare captured terminal output/state with structural assertions or snapshots at 100×28 and 80×24.

Compare termios attributes before/after; assert mouse/alternate-screen/cursor cleanup sequences are emitted and a subsequent shell command receives normal echoed input. Do not count a rendering-only snapshot as proof that mouse input or terminal cleanup works. The user must not need to repair the terminal after exit.

### 16.9 Real-hardware acceptance

**Preparation and authorization:** the user states that the station will be powered on with Bluetooth enabled. Automatically run passive checks when it is reachable. Reversible output tests need explicit test-runner opt-in (`--allow-output-changes` or `MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES=1`) and an operator-established condition that AC/DC loads are safely interruptible and do not power the test computer, Proxmox host, network path, or critical equipment. A historical safe-load experiment or a current 0 W reading is not proof of that condition. Do not silently set the opt-in merely to make tests pass.

Document this preparation once in the hardware-test README. With the precondition/opt-in supplied, execute the test sequence without confirmation prompts at each switch. If it is not supplied, complete all implementation/offline/read-only work and explicitly label the hardware-write acceptance NOT RUN. Do not downgrade the product's implemented control scope.

Run through the **new server's API**, not by simultaneously launching the reference writer:

1. Start one daemon with separate validation paths, explicit device/adapter, and diagnostic logging. Ensure other local probes are closed; do not terminate unrelated user processes.
2. Discover/subscribe and receive validated real samples. Record actual identity, versions, original complete flags, original output booleans, and configuration. A previous research “final state” is not the current baseline.
3. Verify CLI human/JSON status, history recording, WebSocket updates, remote log tail, and runtime debug on/off against this daemon.
4. With authorized safe loads and known ordinary common-lamp mode, traverse the qualified Gray cycle, rotated to the observed original state. Change one switch per API command, require two confirmations, preserve RX `04/08`, and record every admitted command/outcome. End at the exact starting flags. Do not claim to restore an unknown SOS/individual-lamp mode.
5. Exercise actual CLI control commands as part of the cycle where practical; test TUI input-to-command wiring in PTY integration tests and its live display against the real service. Separate physical observation, BLE observation, and simulated GUI tests in the report.
6. Close/reopen CLI/TUI and verify the daemon continues recording and retains one BLE owner. Perform one controlled daemon/session stop/restart without changing outputs; record successful reacquisition.
7. Run a **60-minute** real connected monitoring session with bounded history/logging, status and stream clients, and periodic process memory/CPU/counter measurements. Record actual gaps/reconnects rather than hiding them. This is new application stability validation, not a claim that research already established it.
8. Stop validation cleanly. Confirm the original output state if safe and observable, disconnect the validation daemon unless the user explicitly asked it to remain running, and leave no probe/camera processes started by the test.

For test restoration, change only qualified outputs sequentially while the link is usable and the latest state is fresh and consistent with the expected test progression. Do not blindly restore after unexpected state changes, lost telemetry, suspected protection, or lost connection. Stop and report the last observed state and required manual action. There is no “restore all on” fallback.

No intentional overheating, overload, short circuit, malformed-frame transmission, unknown command probing, or automatic real-host rfkill/BlueZ mutation belongs in acceptance testing. Camera availability is optional; do not reconstruct camera infrastructure. Missing physical observations are reported as such.

The test runner must write machine-readable results and a concise human report with timestamps, versions, actual commands, counts, source/target states, confirmation samples, durations, terminal-test results, resource samples, and PASS/FAIL/NOT RUN classifications. Raw frame evidence from new tests may be kept as DEBUG JSONL under a new validation directory, never in telemetry SQLite and never by overwriting research captures.

### 16.10 Performance and quality gates

These are application targets to measure, not existing hardware guarantees:

- Backend health/status remain responsive during 20-second scans, connection failures, and recording/log activity. Target loopback p95 under 500 ms for status during acceptance load.
- No sustained memory growth proportional to notification count, request count, or disconnected/slow clients. Report server RSS after warmup and throughout the hour; investigate growth above 20 MiB after warmup. Initial backend budget target: at most 160 MiB steady state, leaving room within the operator's tentative 512-MiB LXC allocation. Report environment-dependent deviations rather than claiming a fixed platform footprint.
- No busy-spin polling loops; TUI refresh at most 4 Hz. Measure CPU instead of inventing expected percentages.
- All offline tests pass; imported evidence tests remain intact.
- Ruff formatting/lint and strict mypy checks pass for production Python code. Scope legacy research out of production lint/type gates without hiding its independent tests.
- Target at least 90% branch coverage for protocol/core/command logic and 85% line coverage across production code. Coverage does not substitute for the listed behavior tests; document any unavoidable platform-specific exclusions individually.
- Build and fresh wheel-install tests pass for server-only and client-only extras. Commit the lockfile and tested dependency versions.
- Scan the lock/dependency set for known advisories using an available uv-invoked audit tool; record the tool/date/results. Do not auto-upgrade the validated transport or suppress findings without explanation and retesting.

### 16.11 CI and reproducible commands

Deliver a CI workflow using `uv`, the lockfile, Python 3.12 and 3.14, and no hardware access. Pin reusable CI actions appropriately and give them minimal read permissions. Run lint, typing, offline tests, the separate imported research unittest baseline, package build, and installation smoke tests. Hardware tests require an explicit marker/opt-in and are never run by default test discovery.

Make these development commands work from the repository root after implementation:

```bash
uv sync --locked --extra server --extra cli --group dev
uv run ruff check src frontends tests
uv run ruff format --check src frontends tests
uv run mypy src frontends
uv run pytest -m "not hardware"
uv build
uv run mypowersd check-config --env-file .env
uv run mypowersd --env-file .env
uv run mypowers --env-file .env status --json
cargo build --release --locked --manifest-path frontends/tui/Cargo.toml
install -m 755 frontends/tui/target/release/mypowers-tui .venv/bin/mypowers-tui
uv run mypowers-tui --env-file .env
```

The default CI/offline configuration must explicitly select a simulated/fake transport and temporary storage; it must not inherit a developer's real dotenv or token. Test CLI parsing for the documented argument order. Add equivalent concrete commands for the hardware runner and wheel/deployment verification in the generated documentation.

## 17. Acceptance checklist and handover

### 17.1 Release acceptance

| ID | Acceptance condition |
|---|---|
| A01 | Server, CLI, and interactive TUI are implemented and installable from one repository |
| A02 | BLE is owned only by the server; client-only installs have no hardware dependency |
| A03 | All eight qualified combinations are implemented; only one logical output changes per request |
| A04 | Writes require newly received qualified state, proper bit preservation, full-operation serialization, and two post-write confirmations |
| A05 | Missing/blocked/off infrastructure and unreachable station remain observable via the API; no false phone/thermal diagnoses |
| A06 | Freshness is independent of link state; stale/invalid/old-session data cannot authorize writes |
| A07 | Production JSON/WS API is authenticated and Caddy-compatible; local development works over loopback HTTP |
| A08 | YAML plus explicit dotenv/environment precedence works, secrets are protected, production is fail-closed |
| A09 | SQLite records new live snapshots at the configured interval, with actual receive times and continuity gaps |
| A10 | No raw BLE frame/log table, history aggregation, retention, or partition subsystem is introduced |
| A11 | CLI has Rich one-shot output, clean JSON, operational controls, logs, debug, and documented exit codes |
| A12 | TUI has live data, working mouse/keyboard controls, pending/error states, trends, logs, and reliable terminal cleanup without Textual |
| A13 | Remote application-log tail/filter/follow and runtime DEBUG work without SSH/journald access |
| A14 | Control retries, client loss, duplicate requests, restart, and ambiguous outcomes cannot silently cause replay or optimistic success |
| A15 | Automated tests exercise all required fault cases without mutating the real host |
| A16 | Real read/control/soak validation is executed where authorized/available and honestly classified |
| A17 | Packaging, CI, supervisor/Caddy examples, operations docs, and validation report are delivered |
| A18 | Existing research/license/user changes remain preserved; no secret or local runtime data is committed |

A complete application does not require deploying HomeStack or obtaining an unavailable DNS credential. Distinguish **application implementation complete**, **local/hardware validation complete**, and **production deployment verified** in the handover. A blocked external test must not be mislabeled PASS.

### 17.2 Required final repository deliverables

Deliver executable code, the committed lockfile, optional dependency sets, database migration/schema, unit/integration/terminal/hardware tests, new fixtures with provenance, CI, configuration examples, package-install commands, supervisor/Caddy examples, generated OpenAPI, WS schema documentation, and operations/troubleshooting documentation.

Create `docs/validation/IMPLEMENTATION_VALIDATION.md` plus a machine-readable result file. Include exact test commands/results, supported/tested runtime versions, known externally blocked checks, hardware identity and original/final states, timings and resource observations, and every acceptance ID's status. Preserve the distinction between historical research evidence and new application validation.

The final coding-agent response should state what was implemented, how to run local server/CLI/TUI, where real config/data/logs live, which tests actually ran, the final observed station state, outstanding external deployment inputs, and local commit hashes. Do not deliver only a plan or claim success from source inspection.

## 18. Engineering reference notes

R1–R8 are the project evidence identified in Section 2. The following primary documentation was consulted on 2026-10-04 for implementation choices. It supports library/platform behavior, not additional S300 capabilities. Library versions must still be resolved, locked, and tested during implementation.

| ID | Primary source and relevance |
|---|---|
| E1 | FastAPI lifespan: `https://fastapi.tiangolo.com/advanced/events/` — resource ownership and startup/shutdown |
| E2 | FastAPI WebSockets: `https://fastapi.tiangolo.com/advanced/websockets/` — WS endpoints and dependencies; tutorial token-in-query examples are intentionally not the security policy here |
| E3 | Uvicorn settings: `https://uvicorn.dev/settings/` — process, bind, timeout, and proxy options; one-worker requirement is a MyPowers ownership decision |
| E4 | uv environments: `https://docs.astral.sh/uv/pip/environments/` — environment selection and explicit system installation |
| E5 | Hatch build configuration: `https://hatch.pypa.io/latest/config/build/` — package mapping from separated source directories |
| E6 | aiosqlite: `https://aiosqlite.omnilib.dev/en/stable/` — asynchronous SQLite adapter using a worker thread |
| E7 | Pydantic settings: `https://docs.pydantic.dev/latest/concepts/pydantic_settings/` — validated settings and environment-over-dotenv precedence; this PRD additionally requires YAML and explicit unknown-variable checks |
| E8 | Python time: `https://docs.python.org/3/library/time.html` — monotonic versus wall-clock time |
| E9 | Python zoneinfo: `https://docs.python.org/3/library/zoneinfo.html` — IANA timezones and tzdata availability |
| E10 | SQLite STRICT tables: `https://sqlite.org/stricttables.html` — type/constraint behavior and minimum feature version |
| E11 | SQLite pragmas: `https://www.sqlite.org/pragma.html` — journal, synchronous, busy timeout, integrity, and schema settings |
| E12 | SQLite WAL/backup considerations: `https://www.sqlite.org/wal.html` — database consistency and sidecars; WAL is deliberately not required by this v1 |
| E13 | Python logging handlers: `https://docs.python.org/3/library/logging.handlers.html` — queue/listener and rotating handlers |
| E14 | Rich Live: `https://rich.readthedocs.io/en/stable/live.html` — alternate screen and explicitly controlled refresh |
| E15 | Xterm control sequences: `https://invisible-island.net/xterm/ctlseqs/ctlseqs.html` — SGR mouse and terminal mode handling |
| E16 | Python termios: `https://docs.python.org/3/library/termios.html` — saving/restoring terminal attributes |
| E17 | Caddy automatic HTTPS: `https://caddyserver.com/docs/automatic-https` — DNS-01, renewal, persistent state, outbound-only issuance requirements |
| E18 | Let's Encrypt challenge types: `https://letsencrypt.org/docs/challenge-types/` — public DNS TXT validation |
| E19 | Caddy reverse proxy: `https://caddyserver.com/docs/caddyfile/directives/reverse_proxy` — HTTP and WebSocket proxying |
| E20 | Caddy TLS directive: `https://caddyserver.com/docs/caddyfile/directives/tls` — DNS provider module/configuration requirements |

The architectural boundaries, limits, default thresholds, API paths, SQL schema, test gates, and rollout sequence in this PRD are **new application design decisions**, not claims that the research proved them. Preserve that distinction in the implementation and validation report.
