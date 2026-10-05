# MyPowers Ratatui client

This is the working Rust TUI. Its visual design follows `prototypes/ratatui-ui`,
with a rounded composition, battery gradient, inline independent INPUT/OUTPUT
readings, two-row fixed-scale sparklines, and unboxed AC/DC/lamps controls.
There is no Python/Rich TUI implementation or rendering fallback.

## Run

Start one `mypowersd` separately. From the repository root:

```sh
cargo run --locked --manifest-path frontends/tui/Cargo.toml
```

`.env` is read from the current directory, never a parent directory. Process
environment overrides dotenv; explicit flags override both. `--env-file PATH`
selects another dotenv. Token/CA paths in dotenv are relative to that file.
The default origin is `http://127.0.0.1:8765`. Authentication, HTTPS/WSS with
system roots or `--ca-file`, `--timeout`, `--timezone`, `--utc`, `--no-color`,
`NO_COLOR`, and `--no-mouse` are supported. HTTP redirects and proxies are refused.
Tokens are not put in URLs or printed in connection errors.

Install the native binary:

```sh
cargo install --locked --path frontends/tui
mypowers-tui
# The Python CLI invokes that installed native binary:
mypowers tui
```

For development, build and place the native executable in the existing uv
virtual environment; no Python TUI dependency set is needed:

```sh
cargo build --release --locked --manifest-path frontends/tui/Cargo.toml
install -m 755 frontends/tui/target/release/mypowers-tui .venv/bin/mypowers-tui
uv run mypowers-tui
uv run mypowers tui
```

## Interaction

| Input | Action |
|---|---|
| a / d / l | Request AC / DC / lamps ON or OFF from the displayed snapshot |
| Tab / Shift-Tab | Focus an output |
| Enter / Space | Activate the focused output |
| Left click, released over the same control | Activate that output |
| F2 | Dashboard |
| F3 | Open/close the logs modal |
| F1 / ? | Help |
| f | Cycle minimum log level |
| Up/Down / PageUp/PageDown / mouse wheel / scrollbar drag | Scroll logs; lazily load adjacent pages |
| Left/Right or [ / ] | Previous/next log day |
| r / F5 in logs | Refresh the selected day's archive |
| + / - in logs | Change records per request: 50, 100, 250, 500, 1,000 |
| Home in logs | Load the beginning of the selected day |
| End in logs | Return to today and follow its live bottom |
| r on dashboard | Request connection retry |
| p | Pause/resume daemon BLE acquisition |
| b | Toggle runtime DEBUG override |
| Double-click MYPOWERS | Copy current rendered API snapshot through Wayland `wl-copy` |
| Double-click LOGS title | Copy every currently loaded log record, including rows outside the viewport |
| Esc | Close a modal/help and return to the dashboard |
| q | Open quit confirmation; Enter confirms, Esc cancels |
| Ctrl-Q / Ctrl-C / Ctrl-Z | Quit immediately and restore the terminal |

The dashboard is centered and capped at 94x28 cells; it fits within smaller
terminals down to 60x19 including the status strip. A single borderless status
row sits below the dashboard with one-cell left/right margins, making its maximum
total size 94x29. Hotkeys stay
in the dashboard's bottom border. The logs modal stays inside that frame with two-cell
margins on every side (90x24 when the dashboard has its full size). Opening a
modal dims the dashboard while telemetry updates continue underneath.
Resize invalidates old mouse presses and
hitboxes. Bracketed paste never activates controls. Drawing is limited to 4 FPS;
HTTP and the independent event/log streams run outside the input/render loop.

Observed states only change through telemetry. Missing values are dashes,
and last-known readings remain visible with explicit freshness/daemon status.
Local age uses a monotonic clock. Pending, stale, unavailable, or daemon-denied
states block output requests. The API revalidates UUID/revision and owns command
execution. Each request has a fresh idempotency UUID and a captured explicit
boolean target. Mutations are never automatically replayed after errors or
reconnection. Full command details remain in the API and structured log records;
the status strip shows only concise feedback.
Quitting leaves daemon collection and outputs running.

Several clients can read the same daemon. Other clients' output changes and
command events appear live; the daemon admits only one output operation at once.
No client opens BLE or SQLite or launches/stops the daemon.

Trends retain at most 120 seconds and 512 actual notifications. Time buckets
represent the last sample in each display column, with empty columns across
missing intervals/segments. No synthetic history is generated. Scales remain
0–100 W input and 0–300 W output; larger readings are displayed numerically while
the graph saturates. Logs show history gaps,
effective level, and DEBUG override expiry. Clipboard support is optional and
requires `wl-copy` and an accessible Wayland session.
Log copies are plain text with timestamps in the selected timezone, levels, and
complete messages, without terminal width clipping. Pages not yet loaded are not
included. Copy results appear in the status strip.

The status strip shows one recent action or connection transition, without
command UUIDs, raw flags, or internal reason codes. New live INFO/WARNING/ERROR
log messages can provide operational feedback; DEBUG records and replayed
history never replace the message. Regular telemetry and repeated command states
do not restart its timer. Feedback fades in four two-second color/style stages
and becomes empty after eight seconds. Success uses green, information muted
cyan, warnings yellow, and errors red. The right region is reserved for future
alerts and remains completely empty for now. Long messages are shortened with
an ellipsis while preserving space for right-aligned indicators.
The logs modal footer shows only archive loading/pagination information. Action
results and pending feedback appear exclusively in the status strip. A response
to an earlier log request cannot overwrite newer action feedback.

Permanent age, history health, adapter, and logging diagnostics are absent from
the normal dashboard. They remain available through the API; a future Settings
diagnostics view will present them separately. Full command details remain in
Logs and the API.

The logs modal opens as a stationary archive for today. Days use the selected
IANA timezone (or the local system timezone), including daylight-saving transitions;
API timestamps and the server's records remain UTC. Every HTTP request filters
`since`/`until` and `min_level` on the daemon and returns at most the selected page
size. The client never reads server log files. It caches at most five archive
pages and a separate bounded recent-stream buffer; discarded pages can be fetched
again through opaque cursors. Page size is local to this session until server-side
mutable settings are implemented.

A vertical scrollbar appears when the cached records exceed the visible rows.
Click its track or drag the thumb; Up/Down, PageUp/PageDown and the mouse wheel
also scroll. At the first/last cached row, another scroll loads the adjacent page;
at a completed day's boundary, another scroll enters the previous/next day.
Empty days are navigable. Future days are blocked.

`r`/F5 refreshes the selected day from its beginning without live insertion. New
stream records cannot move a stationary archive or a dragged thumb. Reaching the
bottom of today's completed range or pressing End enables live follow. Scrolling
up disables follow immediately. The header labels ARCHIVE/LIVE explicitly.

Restart both the daemon and TUI after updating this version: the log API now
returns bidirectional pagination metadata. Updating a binary does not update an
already running process. Runtime log-level changes are INFO audit records even
when the selected level suppresses ordinary INFO messages.

## Verify

```sh
cargo fmt --check --manifest-path frontends/tui/Cargo.toml
cargo clippy --locked --manifest-path frontends/tui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path frontends/tui/Cargo.toml
cargo build --locked --manifest-path frontends/tui/Cargo.toml
uv run pytest tests/terminal -q
./scripts/mypowers-hidden-terminal-test.sh /tmp/mypowers-terminal.png
```

PTY tests use an isolated simulated daemon, not the physical station. Xvfb
screenshots do not affect the user's desktop. Visual checks against real data
are observational; they do not activate output controls.
