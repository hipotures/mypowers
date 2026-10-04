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
| F3 | Logs |
| F1 / ? | Help |
| f | Cycle minimum log level |
| Arrows / PageUp / PageDown / mouse wheel | Scroll logs |
| End | Follow latest logs |
| r | Request connection retry |
| p | Pause/resume daemon BLE acquisition |
| b | Toggle runtime DEBUG override |
| Double-click MYPOWERS | Copy current rendered API snapshot through Wayland `wl-copy` |
| q / Esc / Ctrl-C / Ctrl-Z | Quit cleanly |

The terminal must be at least 60x18. Resize invalidates old mouse presses and
hitboxes. Bracketed paste never activates controls. Drawing is limited to 4 FPS;
HTTP and the independent event/log streams run outside the input/render loop.

Observed states only change through telemetry. Missing values are dashes,
and last-known readings remain visible with explicit freshness/daemon status.
Local age uses a monotonic clock. Pending, stale, unavailable, or daemon-denied
states block output requests. The API revalidates UUID/revision and owns command
execution. Each request has a fresh idempotency UUID and a captured explicit
boolean target. Mutations are never automatically replayed after errors or
reconnection. Outcomes include the retained command ID; ambiguous admission
shows its idempotency key. Quitting leaves daemon collection and outputs running.

Several clients can read the same daemon. Other clients' output changes and
command events appear live; the daemon admits only one output operation at once.
No client opens BLE or SQLite or launches/stops the daemon.

Trends retain at most 120 seconds and 512 actual notifications. Time buckets
represent the last sample in each display column, with empty columns across
missing intervals/segments. No synthetic history is generated. Scales remain
0–100 W input and 0–300 W output; larger readings are displayed numerically while
the graph saturates. Logs retain at most 1,000 records and show history gaps,
effective level, and DEBUG override expiry. Clipboard support is optional and
requires `wl-copy` and an accessible Wayland session.

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
