# Implementation validation

Validated on 2026-10-04. Application implementation and local validation are complete. Real-unit
read validation passed; physical control acceptance remains **NOT RUN**. Production deployment
remains **NOT RUN**. These results concern the new application, separately from archived research.
No subagents were used.

The tested final source is `7a2c814c9395a76da3b00fa58cbf840c03a6b7fb`. Initial implementation commit:
`37239a7dbfbcb60e1feaf0080df58a613e39b115`. Baseline:
`52dc2033d9975b15598b36402691c2a92aafdfc8`. The report/results commit adds evidence only.
[results.json](results.json) contains machine-readable counts, versions, original/final states,
all 361 resource samples and every acceptance classification. Working captures are intentionally ignored.

## Environment and dependencies

Linux `7.2.8-1-cachyos`, x86_64, glibc 2.44, QEMU Q35 guest. Tested Python 3.12.14 / SQLite 3.53.1
and Python 3.14.7 / SQLite 3.53.4. Supported minimums are Python 3.12 and SQLite 3.37 on Linux;
the minimum SQLite version and non-Linux systems were not independently tested.

| Package/tool | Tested version |
|---|---|
| mypowers | 0.1.0 |
| bleak | 3.0.2 |
| dbus-fast | 3.1.2 |
| fastapi | 0.142.2 |
| starlette | 1.7.0 |
| uvicorn | 0.54.0 |
| aiosqlite | 0.22.1 |
| websockets | 17.2 |
| rich | 14.3.4 |
| httpx | 0.28.1 |
| pydantic | 2.13.5 |
| python-dotenv | 1.2.4 |
| PyYAML | 6.0.3 |
| ruff | 0.16.10 |
| mypy | 2.4.0 |
| pytest | 9.1.1 |
| coverage | 7.16.2 |
| pip-audit | 2.10.1 |

Caddy: `v2.11.7 h1:yj0Y4fYZGPkSvibBJ1sTWE33xC0fxztVyXEW5iIdUT4=`. The official release archive's SHA512 was checked before use.
Transport stays pinned to Bleak 3.0.2 and dbus-fast 3.1.2. `uv.lock` and hashed deployment exports
agree (only export-command output-path comments differ on re-export).

## Executed automated validation

| Check | Actual result |
|---|---|
| Complete suite, Python 3.12.14 | 280 PASS, 0 skipped/failures, 89.84s |
| Complete suite, Python 3.14.7 | 280 PASS, 0 skipped/failures, 92.86s |
| Preserved research unittests | 35 PASS on each interpreter |
| Ruff lint / formatting | PASS; 47 Python files formatted |
| Strict mypy | PASS; 26 production files |
| Production line coverage | 91.51% (target 85%) |
| Protocol/core branch coverage | 95.38% (target 90%; 124/130) |
| Source/wheel build and fresh installs | PASS; server-only/client-only on both interpreters, outside checkout |
| Dependency advisory audit | PASS; 65 applicable runtime/dev dependencies, no known findings |
| Actual Caddy HTTPS/WSS | PASS; explicit private CA accepted, default trust rejected, no trust-store modification |
| API during 20-second fake scan | PASS; 40 requests, p95 4.04ms (target 500ms) |

Each suite emitted one Starlette TestClient deprecation warning about httpx. It did not cause a failure.
Earlier validation found stale retry-delay display after reacquisition and one failing Python 3.14
PTY case: a late WS command event overwrote the HTTP outcome's format and command ID. Four subsequent
Python 3.12 PTY failures exposed early WS confirmation while local polling remained pending.
The local operation now owns its notice until completion; early/late event ordering and stale retry
display are checked before the final complete suites recorded above.
Coverage uses the distinct line and branch denominators, rather than the combined display percentage.
The only excluded lines are default coverage exclusions for type-only Protocol ellipsis declarations
and adjacent blank lines in `bluetooth/transport.py` and `core/__init__.py`; no platform/runtime
failure path was explicitly excluded. Remaining uncovered paths are reported, not claimed tested.

Actual final commands (from repository root):

```bash
UV_CACHE_DIR=/tmp/mypowers-uv-cache uv lock --check --offline
.venv/bin/ruff check src frontends tests
.venv/bin/ruff format --check src frontends tests
.venv/bin/mypy src frontends
MYPOWERS_CADDY_TEST_BINARY=/tmp/mypowers-caddy/caddy \
  MYPOWERS_TLS_RESULT_FILE=docs/validation/work/tls.json \
  MYPOWERS_API_LOAD_RESULT_FILE=docs/validation/work/api-load.json \
  .venv/bin/pytest -m 'not hardware' --cov \
  --cov-report=json:docs/validation/work/coverage.json \
  --junitxml=docs/validation/work/pytest314.xml
MYPOWERS_CADDY_TEST_BINARY=/tmp/mypowers-caddy/caddy \
  .local/test312/bin/pytest -m 'not hardware' \
  --junitxml=docs/validation/work/pytest312.xml
.venv/bin/python tests/check_coverage.py docs/validation/work/coverage.json
UV_CACHE_DIR=/tmp/mypowers-uv-cache uv build --offline
UV_CACHE_DIR=/tmp/mypowers-uv-cache .venv/bin/python tests/verify_wheel.py \
  dist/mypowers-0.1.0-py3-none-any.whl --offline \
  --result-file docs/validation/work/wheel314.json
UV_CACHE_DIR=/tmp/mypowers-uv-cache .local/test312/bin/python tests/verify_wheel.py \
  dist/mypowers-0.1.0-py3-none-any.whl --offline \
  --result-file docs/validation/work/wheel312.json
```

Before the Python 3.12 wheel cache was populated, its offline install correctly failed for an absent
dbus-fast wheel. The same hashed install was run with network enabled, then passed offline.
Local socket/D-Bus tests ran outside the restrictive tool sandbox; they require neither host-service
changes nor hardware in default pytest discovery. Ordinary CI may skip the optional Caddy binary
test; this explicit release run skipped nothing.

From `docs/research/s300`, each of these passed 35 tests:

```bash
/home/user/DEV/mypowers/.venv/bin/python -m unittest \
  test_control_allpowers.py test_s300_matrix.py test_s300_connection_lab.py
/home/user/DEV/mypowers/.local/test312/bin/python -m unittest \
  test_control_allpowers.py test_s300_matrix.py test_s300_connection_lab.py
```

The advisory audit covered applicable locked runtime plus dev packages, with no suppression:

```bash
UV_CACHE_DIR=/tmp/mypowers-uv-cache uv export --offline --locked --all-extras \
  --group dev --no-emit-project --no-hashes \
  --output-file /tmp/mypowers-audit-all-requirements.txt
UV_CACHE_DIR=/tmp/mypowers-uv-cache uv run --offline pip-audit --no-deps --disable-pip \
  -r /tmp/mypowers-audit-all-requirements.txt --format json \
  --output docs/validation/work/audit-all.json
```

Audit result depends on advisory availability on this date; foreign-platform marker exclusions
were honored. Installation uses the committed hashed exports. The audit's no-hash export only
supplies pinned versions to the advisory service; `--disable-pip` prevents package installation.

## Behavior exercised

Protocol fixtures retain provenance from all eight captured states. Tests cover exact nine-byte
commands, all 24 directed one-output edges, checksum/envelope/range failures, zero and uint16
boundaries, unsupported profiles and transport allowlisting. Command tests exercise a newly
received post-admission state, complete-operation serialization, no-change without writes,
two full-state confirmations, revision/UUID guards, busy/dedup/body conflicts, deadlines,
source-then-target/unrelated changes, stale and old-session events, client loss, queue overflow,
registry bounds/expiry and cancellation before/after possible send. Ambiguous outcomes never replay.

Injected infrastructure tests cover bus/service/permission/adapter/rfkill/name/GATT/scan/setup
and cleanup failures, restoration, silence/recovery, coalesced retry/pause/resume, single ownership
and unexpected essential-task exits. Storage tests cover constraints, unavailable/read-only/full
storage recovery, bounded queues, no cached restamping, discontinuities, collision ordering,
stable pagination and refusal of future/unversioned/corrupt files. Migration experiments use
temporary Online Backup API snapshots and integrity checks.

API tests exercise auth, minimal public health, Host/Origin checks, strict bodies/body limits,
typed errors/OpenAPI, first-message WS authentication/deadline, heartbeat, state/log streams,
and actual Uvicorn/shared client use. Diagnostics tests cover redaction, DEBUG expiry/reversion,
rotation, pagination expiry, malformed final lines, failure rings, bounded records/queues and
operational-record priority over DEBUG. Installed CLI requests and error/uncertainty behavior
are tested independently of frontend internals.

Real PTY tests exercise mouse/keyboard output intentions against the simulated daemon, split
escape input, paste suppression, resize, color/compact rendering and clean q/Ctrl+C/Ctrl+Z/SIGTERM
exit, followed by a working shell read/echo. Separate failure tests check renderer/API loss cleanup.
A six-second passive TUI against the real station displayed live/battery values and restored
termios/mouse/paste modes with exit 0 and no traceback. No physical output control was activated.

## New real-hardware evidence

Qualified station `2A:02:01:48:6B:D0`, GATT-readable `AP S300 V2.0`; Actions adapter
`F4:4E:FC:A1:CB:FF`, resolved to `hci2` during these runs. No firmware/hardware revision was
invented from the readable name. All test data/log/runtime paths were isolated under
`.local/validation/`; original research captures were not modified.

The hour used a temporary passive driver (`.venv/bin/python /tmp/mypowers-hardware-soak.py`),
starting `.venv/bin/mypowersd --env-file .local/validation/hardware.env`, keeping a WS subscriber
and querying status/resources every ten seconds. Started 2026-10-04T17:27:16.366+00:00; ended
2026-10-04T18:27:34.816+00:00; elapsed 3618.45s, including at least 60 minutes
after the first live observation. There was one expected HTTP ConnectError before listener startup.
Every captured status thereafter was connected/live; 2719 valid notifications,
0 rejected frames, 1 initial acquisition, 2722 WS messages and one
continuous history segment with 359 recorded rows.

After five-minute warmup: RSS 65.59 MiB, maximum
66.70 MiB, growth 1.10 MiB
(below the 20 MiB investigation threshold and 160 MiB backend target). CPU consumed
0.52% of one logical core, calculated from process
CPU ticks over wall time. Status p95 was 5.79ms. These are
observations in this guest and load, not guaranteed LXC performance.

The hour daemon loaded an earlier application revision before the final diagnostics/freshness/
retry-display corrections; it was not hot-reloaded. **A full hour on the final corrected revision
was NOT RUN.** The final corrected source was separately started and passed installed CLI
status/historical/log/debug commands, WS snapshot, explicit session release/reacquisition and
a ten-second passive monitor. Actual final runner command:

```bash
.venv/bin/python tests/hardware/run_acceptance.py \
  --env-file .local/validation/hardware.env \
  --output .local/validation/retry-read --seconds 10
```

That run started 2026-10-04T18:34:00.341Z and finished 2026-10-04T18:34:28.857Z. Every installed CLI
command returned 0. The recovered status clears `retry_in_seconds`. Two continuity segments
showed session reacquisition. Both daemons completed graceful application shutdown and then
Uvicorn re-raised SIGTERM (process status -15); no forced termination was needed. BlueZ's object
tree after shutdown contained only adapters and no station device object. No probe remains running.

| Observation | Initial hour | Final corrected read |
|---|---|---|
| Received UTC | 2026-10-04T17:27:20.888Z | 2026-10-04T18:34:25.762Z |
| Complete flags | 0x0C | 0x0C |
| AC / DC / common lamps | off / off / off | off / off / off |
| Battery | 85% | 78% |
| Aggregate input / output | 0 / 5 W | 0 / 3 W |
| Reported remaining time | 1388 min | 1796 min |

Physical writes sent: **0**. Source/target control transitions and confirmation samples: **NOT RUN**
on hardware. No safe-load/common-lamp declaration or write opt-in was supplied; low wattage does
not establish safety. Physical electrical/lamp observations, deliberate phone contention,
station-power interruptions and host adapter/service mutations were not performed. Faults are
covered with injected offline transports rather than altering the user's host.

Online Backup API snapshots of the hour and both short-read databases passed
`PRAGMA integrity_check` (`ok`), schema version 1, only `devices`/`telemetry` tables. No plain
database copy or migration experiment touched the originals. The final snapshot contains
3 telemetry rows and 2 continuity segments.
Raw DEBUG evidence stays in ignored JSONL logs; SQLite contains neither raw frames nor logs.

Reproduce the hour on the final source using the committed runner with `--seconds 3600`; see
[hardware preparation](../../tests/hardware/README.md). Physical control additionally requires
the explicit safe-load, ordinary-lamp and write flags described there. Do not run simultaneous
daemons/probes or replay an uncertain operation.

## Acceptance checklist

PASS refers to the stated implementation/local evidence. A16 retains explicit partial hardware
acceptance; A17 delivery does not claim an external production deployment.

| ID | Status | Evidence / limitation |
|---|---|---|
| A01 | PASS | Daemon, CLI and interactive Rich TUI; wheel installation on both runtimes. |
| A02 | PASS | Only daemon owns BLE; client wheels exclude all hardware/server dependencies. |
| A03 | PASS | All eight exact encodings and 24 directed single-output edges tested offline. |
| A04 | PASS | Post-admission read, serialized single send and two full-state confirmations tested offline. |
| A05 | PASS | Structured BlueZ/adapter/station failure observations; no automatic host mutations. |
| A06 | PASS | Monotonic freshness, invalid data, session isolation, revision and qualification tests. |
| A07 | PASS | Auth/Host/Origin/body/WS boundaries and actual private-CA Caddy HTTPS/WSS test. |
| A08 | PASS | Strict YAML, explicit dotenv/env precedence, private tokens and production fail-closed checks. |
| A09 | PASS | Actual receive-time recording, new samples, gaps, stable cursors and live SQLite snapshots. |
| A10 | PASS | Only devices/telemetry tables; no raw BLE/log tables or aggregation/retention subsystem. |
| A11 | PASS | Installed Rich/JSON CLI commands, NDJSON stream contract and documented exit codes. |
| A12 | PASS | PTY mouse/keyboard/resize/paste/exit/failure tests plus passive real-unit live TUI. |
| A13 | PASS | Remote log queries/follow, bounded failure fallback, rotation and runtime DEBUG tests. |
| A14 | PASS | Busy/dedup/client loss/restart/overflow/send timeout/uncertainty tests; no automatic replay. |
| A15 | PASS | 280 isolated automated cases on each runtime; faults injected without altering host services/devices. |
| A16 | PARTIAL | Real reads, 60-minute passive soak and reacquisition passed; real output writes/physical observations NOT RUN. |
| A17 | PASS | Lock, packaging, CI, config, supervisors, Caddy, OpenAPI and validation artifacts delivered; production deployment NOT RUN. |
| A18 | PASS | Research/license unchanged; original untracked input preserved; no private/runtime files in commits. |

## Handover and external checks

Use the [README](../../README.md) for local `uv sync`, explicit `.env`, daemon/CLI/TUI commands.
Development config is `config/development.yaml`; after copying `.env.example` to `.env`, state is
under `.local/dev/data`, `.local/dev/logs`, `.local/dev/run`. Validation's actual private config and
captures are `.local/validation/hardware.env`, `.local/validation/` and `docs/validation/work/`.
No implicit dotenv is read. Installed defaults follow XDG; production examples select
`/etc/mypowers`, `/var/lib/mypowers`, `/var/log/mypowers`, `/run/mypowers`.

Production needs the actual DNS provider, provider-specific Caddy DNS-01 module and credential,
target LXC/controller/BlueZ/service permissions, LAN DNS/reachability, private API token and
persistent certificate/backup stores. Those external inputs were unavailable and were not
provisioned. Certificate issuance/renewal, target LXC Bluetooth, supervisor activation and LAN
client verification are **NOT RUN**. Caddy's local test CA is never the production fallback.

All task changes are committed. The supplied `docs/MYPOWERS_PRD.md` was untracked at start and
remains untracked user input; `docs/PRD.md` is its byte-identical committed copy. All 60 tracked
research files and LICENSE remain unchanged from baseline. No private config/token/database/log,
working CA, runtime capture or unrelated user change was committed. `git diff --check` passes for
new implementation files; imported PRD alone retains its intentional Markdown two-space breaks.
