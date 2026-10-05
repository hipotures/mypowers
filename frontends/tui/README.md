# MyPowers Ratatui client

This is the working Rust TUI. Its visual design follows `prototypes/ratatui-ui`,
with a rounded composition, battery gradient, inline independent INPUT/OUTPUT
readings, two-row sparklines or a shared chart with four plot rows, and unboxed AC/DC/lamps controls.
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
| Esc | Close a modal/help and return to the dashboard; no action on the dashboard |
| q | Open quit confirmation; Enter confirms, Esc cancels |
| Ctrl-Q | Quit immediately and restore the terminal |
| a / d / l on dashboard | Request AC / DC / lamps ON or OFF from the displayed snapshot |
| Tab / Shift-Tab on dashboard | Focus an output |
| Enter / Space on dashboard | Activate the focused output |
| Left click, released over the same dashboard control | Activate that output |
| F3 on dashboard | Open the logs modal |
| s on dashboard | Open read-only Settings / Diagnostics |
| t on dashboard | Cycle average per bucket: 10 seconds, 60 seconds, 1 hour |
| g on dashboard | Switch two sparklines / shared line chart (session only) |
| F1 / ? on dashboard, in logs, or in settings | Help for the active context |
| f in logs | Cycle minimum log level |
| Up/Down / PageUp/PageDown / mouse wheel / scrollbar drag in logs | Scroll records; lazily load adjacent pages |
| Left/Right or [ / ] in logs | Previous/next log day |
| + / - in logs | Change records per request: 50, 100, 250, 500, 1,000 |
| Home in logs | Load the beginning of the selected day |
| End in logs | Return to today and follow its live bottom |
| r on dashboard | Request connection retry |
| p on dashboard | Pause/resume daemon BLE acquisition |
| b in logs or settings | Toggle runtime DEBUG override |
| Double-click MYPOWERS | Copy current rendered API snapshot through Wayland `wl-copy` |
| Double-click LOGS title | Copy every currently loaded log record, including rows outside the viewport |

Only q, Ctrl-Q, and Esc are global. Other shortcuts belong to the active view;
modal input never dispatches dashboard actions. F2 and log refresh shortcuts are
removed. Typed Ctrl-C/Ctrl-Z are ignored; external SIGINT/SIGTERM still trigger
terminal restoration. Help lists the shortcuts of the view that opened it.

The dashboard is centered and capped at 94x28 cells; it fits within smaller
terminals down to 60x19 including the status strip. A single borderless status
row sits below the dashboard with one-cell left/right margins, making its maximum
total size 94x29. Hotkeys stay
in the dashboard's bottom border. The logs modal stays inside that frame with two-cell
margins on every side (90x24 when the dashboard has its full size). Opening a
modal dims the dashboard while telemetry updates continue underneath.
Help and Settings are overlays using the same margins. Settings currently shows
read-only timezone/page-size preferences and actual runtime diagnostics: daemon,
station phase, BLE adapter, telemetry age/state, history, and effective logging.
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

Terminal cleanup also runs on I/O errors and panics. A panic on the UI thread,
a worker thread, or an async task restores the terminal, reports the panic, and
exits with code 101. The client cannot continue rendering after a worker panic
has restored cooked mode. Signal listeners are registered before raw mode;
registration errors stop startup instead of silently disabling signal cleanup.

Several clients can read the same daemon. Other clients' output changes and
command events appear live; the daemon admits only one output operation at once.
No client opens BLE or SQLite or launches/stops the daemon.

Graphs use server-computed SQLite averages from `GET /api/v1/history/aggregates`.
Press `t` on the dashboard to cycle **10s → 60s → 1h per bar**; the footer shows the
selected interval. INPUT/OUTPUT numeric labels always show current telemetry,
independently of historical averages. Selection is local to each TUI session.
Sparkline is the default. Press `g` to switch to a shared built-in
Ratatui `Chart` with Braille line datasets: INPUT green, OUTPUT cyan. Colored
INPUT/OUTPUT labels identify the series without a boxed legend. A terminal cell
has one foreground color. Each series uses the built-in Chart renderer; a
small Buffer compositor unions their Braille patterns, so neither line erases
the other. Cells shared by both series use a neutral foreground. Recording gaps break each line; measured zeros remain points on the baseline. The chart uses
the full content width, with seven columns reserved for the power axis; each
remaining plot column requests one aggregate bucket. The vertical axis shows
zero, half-scale and maximum, in W. Four plot rows remain above two additional
rows for the horizontal axis and time labels. Time labels follow the selected
timezone: HH:MM:SS for 10s buckets, HH:MM for 60s, and month-day/hour for 1h.
The endpoint labels identify the oldest and latest bucket starts, not the time
of the last observation. Axes remain visible during idle. When both channels are idle it shows one shared idle marker.
Visualization selection is local to the current process; no settings API,
database persistence or config-file writes are introduced.

For 43 columns, the visible history spans about 7 minutes, 43 minutes or 43 hours,
including the unfinished current bucket. UTC epoch boundaries keep completed
bars stable between redraws and shift the window by whole columns.

On startup, reconnect, resize or interval change, the client requests only the
visible buckets (maximum 256). Successful queries refresh every five seconds;
failed queries retry after two seconds and keep the existing graph without
blocking live readings or controls. Changing interval clears old averages and
cancels obsolete requests; responses from an older daemon, size or selection
cannot replace current history. Disabled/degraded server history is not queried.
The frontend does not download or aggregate raw sample pages.

Each returned bucket contains SQL AVG(INPUT), AVG(OUTPUT) and COUNT(*). Measured
zeros participate in the mean; intervals without observations stay empty.
Persisted samples follow the server's history interval (10 seconds by default).
The current bucket's average can change as additional recorded samples arrive;
no interpolation or invented history fills recording gaps. Fractional averages
are retained, including positive values below 1 W. Both visualizations start at
0–100 W and double the maximum until visible averages and current readings fit:
100 → 200 → 400 W, etc. The scale returns to a smaller step when the peak leaves
the window. Sparklines scale independently; the shared chart uses one maximum
for INPUT and OUTPUT. The current range appears above each plot. Positive
sparkline averages occupy at least one eighth-cell tick; zero stays empty. Malformed, unordered, mismatched or oversized responses are rejected
as a complete request rather than partially displayed.
Clipboard support is optional and
requires `wl-copy` and an accessible Wayland session.
Log copies are plain text with timestamps in the selected timezone, levels, and
complete messages, without terminal width clipping. Pages not yet loaded are not
included. Copy results appear in the status strip.

When a live INPUT or OUTPUT reading is zero and its selected history window has
no positive averages left, that graph shows a dim eleven-cell `·····○·····` idle
track. The marker moves one cell every two seconds and reverses at the ends.
Each graph decides independently; nonzero readings immediately restore the real
sparkline. Existing positive averages delay the idle track until they leave the
selected window. Missing, stale, or disconnected telemetry never animates it.
The track uses Ratatui Paragraph/Line/Span widgets and the existing render loop.

The status strip shows one recent action or connection transition, without
command UUIDs, raw flags, or internal reason codes. New live INFO/WARNING/ERROR
log messages can provide operational feedback; DEBUG records and replayed
history never replace the message. Regular telemetry and repeated command states
do not restart its timer. Feedback fades in four two-second color/style stages
and becomes empty after eight seconds. Success uses green, information muted
cyan, warnings yellow, and errors red. The right region is reserved for future
alerts, displaying only nonzero counts supplied to the UI. While Logs is open,
it also shows contextual runtime metadata: `Log: INFO`,
with `| until YYYY-MM-DD HH:MM:SS` in the selected timezone only when an override
has a future expiry. It is empty on the normal dashboard until alerts exist.
Long messages are shortened with
an ellipsis while preserving space for right-aligned indicators.
The logs modal footer shows only archive loading/pagination information. Action
results and pending feedback appear exclusively in the status strip. A response
to an earlier log request cannot overwrite newer action feedback.
The two-row header contains day navigation and archive/live mode, timezone,
`Filter ≥ LEVEL`, and page size. Log controls sit in the modal's bottom border;
the dashboard's bottom border shows close/help/quit while the modal is open.
Removing the two old instruction/diagnostic header rows leaves 19 visible records
at the maximum dashboard size.

Permanent age, history health, adapter, and logging diagnostics are absent from
the normal dashboard and appear in Settings / Diagnostics. Full command details
remain in Logs and the API. Mutable settings and server alert tracking are not
implemented by the snapshot tool.

The logs modal opens as a stationary archive for today. Days use the selected
IANA timezone (or the local system timezone), including daylight-saving transitions;
API timestamps and the server's records remain UTC. Every HTTP request filters
`since`/`until` and `min_level` on the daemon and returns at most the selected page
size. The client never reads server log files. It caches at most five archive
pages and a separate bounded recent-stream buffer; discarded pages can be fetched
again through opaque cursors. Page size is local to this session until server-side
mutable settings are implemented.

Live follow refreshes its cached page range as records accumulate. If HTTP
refreshes keep failing while the log stream still works, loaded rows stop growing
at five page sizes plus a 1,000-record catch-up allowance; the separate recent
stream buffer also stays limited to 1,000 records. Loaded rows and opaque cursors
remain intact. A successful latest-page refresh restores current records and
merges arrivals received during that request.

A vertical scrollbar appears when the cached records exceed the visible rows.
Click its track or drag the thumb; Up/Down, PageUp/PageDown and the mouse wheel
also scroll. At the first/last cached row, another scroll loads the adjacent page;
at a completed day's boundary, another scroll enters the previous/next day.
Empty days are navigable. Future days are blocked.

Home loads the selected day's beginning. There is no refresh button or shortcut:
the footer counts new stream records, and End fetches today's latest records and
resumes live follow. New stream records cannot move a stationary archive or a
dragged thumb. Reaching the
bottom of today's completed range or pressing End enables live follow. Scrolling
up disables follow immediately. The header labels ARCHIVE/LIVE explicitly.

Restart both the daemon and TUI after updating this version: the log API now
returns bidirectional pagination metadata. Updating a binary does not update an
already running process. Runtime log-level changes are INFO audit records even
when the selected level suppresses ordinary INFO messages.

## Verify

Generate deterministic SVG images from the same production Ratatui renderer,
without a daemon or terminal:

```sh
cargo xtask ui-snapshots
cargo xtask ui-snapshots --check
```

See [the snapshot developer note](../../artifacts/ui/README.md) for scenes, fixed
time/data, exporter limitations, and optional PNG inspection. The shared library
contains the production renderer; `xtask` contains only fixture data, cell-to-SVG
export, and file generation.

For a dependency-free offline render benchmark with fixed application state:

```sh
cargo bench --locked --manifest-path frontends/tui/Cargo.toml --bench render
```

It measures the production renderer through TestBackend at 120x30, using live,
idle, and Logs scenes. The output reports per-frame median/min/max across five
rounds. Measurements include Ratatui buffer diffing, exclude real terminal I/O,
and vary with system load; compare before/after runs on the same machine.
Ratatui's `layout-cache` feature is explicitly enabled alongside Crossterm; its
bounded built-in cache avoids solving unchanged layouts again on every frame.

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
The `terminal_cleanup` example is a subprocess fixture for PTY tests of the
production terminal guard: normal exit, main/worker/task panics, and setup writes
to a broken pipe. It does not connect to a daemon or render a separate UI.
