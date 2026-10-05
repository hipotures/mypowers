# Ratatui implementation review

Review window: 2026-10-05 02:07–08:07 UTC. Work is performed without subagents.
The scope is correctness, responsiveness, resource use, testing, and terminal
cleanup. Dashboard and modal appearance must remain unchanged.

## Documentation map

The [Ratatui documentation](https://ratatui.rs/tutorials/) is organized into:

- Getting Started: installation and feature flags.
- Tutorials: Hello Ratatui, Counter App, JSON Editor, and videos.
- Examples: applications, layout, styles, and individual widgets.
- Concepts: widgets, layouts, events, rendering, application patterns, and backends.
- Recipes: layout, rendering, widgets, testing, and application infrastructure.
- Ecosystem, FAQ, release highlights, showcase, templates, references, and contributor guides.

The map has been inspected. Detailed review starts with the sections corresponding
to the production TUI, then expands to tutorials and adjacent practices. Unrelated
platforms and showcase applications are context rather than implementation requirements.

## Reviewed topics and current findings

- [Immediate rendering](https://ratatui.rs/concepts/rendering/) and
  [buffer rendering](https://ratatui.rs/concepts/rendering/under-the-hood/): the
  application already draws a complete frame using Ratatui widgets and Buffer;
  terminal output diffing is handled by Ratatui.
- [Event handling](https://ratatui.rs/concepts/event-handling/) and
  [terminal/event handler recipe](https://ratatui.rs/recipes/apps/terminal-and-event-handler/):
  terminal input stays on the UI thread and network operations use bounded channels
  on a separate runtime. The existing structure does not require a new UI framework.
- [Panic hooks](https://ratatui.rs/recipes/apps/panic-hooks/): the current terminal
  guard restores raw mode, mouse capture, paste mode, alternate screen, and cursor.
  Failure-path and worker/task-panic behavior is covered by subprocess PTY tests;
  cycle 4 fixed continued execution after panic restoration.
- [Snapshot testing](https://ratatui.rs/recipes/testing/snapshots/): the project
  already shares its production renderer with TestBackend snapshots. A non-writing
  comparison command closes the gap between generating previews and enforcing
  visual preservation without adding insta or a second layout.

## Verification cycles

### 1. Make visual preservation checkable

Before editing, generated all 15 scenes and saved their SVGs and SHA-256 sums.
Rasterized and inspected the live dashboard using `rsvg-convert`, without desktop
access. Added `cargo xtask ui-snapshots --check`: it reports every changed/missing
scene, fails nonzero, and never overwrites or creates files. Added regression tests
for successful comparison and failed comparison without filesystem mutation.

Validation: 9 xtask tests passed; Clippy with warnings denied, formatting, and diff
checks passed. The check command verified all 15 scenes. Regenerated SVG SHA-256
sums match the before set exactly; rasterized live dashboard and Logs PNGs also
match byte-for-byte. Inspected the dashboard before and Logs after. No UI change.

### 2. Remove repeated work in dashboard rendering

Reviewed the [Sparkline example](https://ratatui.rs/examples/widgets/sparkline/)
and the locally installed 0.30.2 widget implementation. Its `data` method accepts
an iterator, so the application need not materialize styled bars first. Freshness
is now evaluated once per dashboard frame, preventing its label, bars, and output
states from using different decisions as telemetry reaches its freshness boundary.
Idle rendering no longer allocates or populates a graph vector that it immediately
discards. Fixed graph scales and the existing idle-history rule are unchanged.

Added a dependency-free offline benchmark using the production renderer and
TestBackend, a fixed clock, 40 history samples, and a 2,000-record log archive:

```sh
cargo bench --locked --manifest-path frontends/tui/Cargo.toml --bench render
```

Five rounds of 5,000 frames at 120x30 produced these median timings on the review
machine (microseconds per frame; TestBackend includes buffer diff/flush work):

| Scene | Before | After |
| --- | ---: | ---: |
| Live dashboard | 897.75 | 873.76 |
| Idle dashboard | 886.01 | 877.36 |
| Logs modal | 1223.41 | 1223.30 |

These are small improvements, with system-load variation especially visible in
the Logs measurements. They do not establish a broad terminal performance gain.
The simplification removes repeated freshness decisions and unused idle work.

Validation: all 30 TUI tests passed, including the local HTTP test rerun with
loopback access after the sandbox denied socket creation. Clippy with warnings
denied passed. All 15 saved SVGs passed `--check`; regenerated SHA-256 sums match
the before set exactly. Live dashboard PNGs match byte-for-byte and the after
image was inspected. No UI change.

The [Hello tutorial](https://ratatui.rs/tutorials/hello-ratatui/),
[Counter update/render/testing sections](https://ratatui.rs/tutorials/counter-app/basic-app/),
[JSON Editor main loop](https://ratatui.rs/tutorials/json-editor/main/), and
[Elm architecture overview](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/)
have also been reviewed. Existing App/effect/renderer separation and contextual
input routing already fit these principles; no replacement framework is needed.

### 3. Enforce renderer snapshots in offline CI

The existing workflow tested the TUI but did not test the SVG exporter or compare
saved scenes. Added xtask formatting, Clippy, tests, and `ui-snapshots --check` to
both existing matrix entries. A job-level Cargo target directory reuses production
dependency builds and retains the binary location required by PTY tests.

Validation: parsed the workflow YAML and verified that CI checks rather than
regenerates artifacts. Ran its added Cargo commands locally with the same shared
target directory: 9 xtask tests passed, formatting and Clippy passed, and all 15
scenes matched. Before/after SVG hashes and Logs PNG bytes match exactly. Remote
GitHub Actions execution has not been claimed or triggered.

### 4. Stop safely after worker panics

Reviewed the [panic-hook recipe](https://ratatui.rs/recipes/apps/panic-hooks/),
[Counter error handling](https://ratatui.rs/tutorials/counter-app/error-handling/),
and Tokio's [task panic behavior](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html).
A production-guard subprocess fixture reproduced a failure for both a native
worker panic and a Tokio task panic: terminal restoration ran, but execution
continued and the process exited successfully (0). Catching a worker's panic
must not leave the TUI running after restoration. The existing panic hook now
restores the terminal, calls the previous diagnostic hook, and exits with 101.

Registered SIGINT/SIGTERM listeners synchronously within the runtime context,
before entering raw mode. Registration errors now stop startup rather than
silently dropping the signal task. Install the terminal guard before spawning
network workers so they cannot panic before cleanup has been installed.
Tokio documents [signal registration](https://docs.rs/tokio/latest/tokio/signal/unix/fn.signal.html)
and [entering the runtime context](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#method.enter).

Validation: 18 isolated PTY tests passed, including five new cleanup cases:
normal exit, main-thread panic, worker panic, async-task panic, and a broken-pipe
write during setup. They verify original termios restoration; panic cases also
verify mouse/alternate-screen/cursor cleanup and exit 101. Existing signal,
controls, paste, resize, multi-client, log, and TLS tests passed against simulated
daemons. All 30 Rust tests, Clippy with warnings denied, formatting, and Ruff
passed. All 15 SVGs pass `--check` and regenerated hashes match the before set;
live dashboard PNGs match byte-for-byte and the after image was inspected.

### 5. Validate streamed logs before passing them to the UI

The HTTP archive path checked record timestamps, unsigned sequences, UUID
identities, and message strings, while the WebSocket path forwarded any non-null
JSON value. A loopback WebSocket regression reproduced this discrepancy: an array
reached the UI event channel instead of producing a log-schema error. Both paths
now share the existing archive validation, and malformed stream records fail
before queueing. Reconnection still belongs to the existing stream worker.
Retained records may correctly identify a previous daemon instance; their UUID
does not have to equal the current WebSocket envelope's instance.

Validation: all 32 Rust tests passed, including two new WebSocket tests covering
11 malformed/missing-data cases and an accepted retained record from a previous
daemon instance. Targeted PTY archive/pagination/live-follow and status-feedback
tests passed against simulated daemons (2 tests). Clippy with warnings denied,
formatting, and diff checks passed. All 15 SVGs passed `--check`; regenerated
hashes and Logs PNG bytes match the before set exactly. Inspected the after PNG.

Reviewed the [layout concepts](https://ratatui.rs/concepts/layout/),
[dynamic-layout recipe](https://ratatui.rs/recipes/layout/dynamic/),
[Block recipe](https://ratatui.rs/recipes/widgets/block/),
[Paragraph example](https://ratatui.rs/examples/widgets/paragraph/), and
[Scrollbar example](https://ratatui.rs/examples/widgets/scrollbar/). The current
layout uses bounded areas, integrated Block titles, Paragraph text, and the
stateful Scrollbar as intended. No layout or cosmetic replacement is warranted.

### 6. Enable Ratatui's bounded layout cache

The [0.30.2 feature guide](https://ratatui.rs/installation/feature-flags/)
identified a performance issue in the dependency configuration: disabling all
defaults also disabled `layout-cache`. The local feature tree confirmed it was
absent, and the installed `Layout::split_with_spacers` implementation consequently
ran the layout solver on every call. Explicitly enabled this Ratatui feature;
the existing layout/rendering code stays unchanged. The default thread-local LRU
is bounded to 500 entries and keys on both the area and Layout, including its
constraints, so resizing selects the correct results. No application cache,
invalidation layer, or cache-size setting was added.

The same release benchmark, fixtures, dimensions, and five rounds of 5,000 frames
produced these medians (microseconds per frame):

| Scene | Before cache | With cache | Ratio |
| --- | ---: | ---: | ---: |
| Live dashboard | 887.95 | 124.99 | 7.10x |
| Idle dashboard | 874.05 | 119.46 | 7.32x |
| Logs modal | 1208.53 | 265.93 | 4.54x |

This is a substantial reduction in TestBackend frame cost, not a claim about
total application CPU or real terminal throughput. The draw cadence remains
4 FPS. Both lockfiles gained only Ratatui's `critical-section` dependency;
resolution/builds used the existing offline crate cache.

Validation: all 32 Rust tests and 9 xtask tests passed. Four native PTY cases
passed controls, resize, modal transitions, paste, and terminal restoration
against simulated daemons. Formatting, Clippy with warnings denied, and diff
checks passed. All 15 SVGs pass `--check`; regenerated SVG hashes and live
dashboard PNG bytes match the before set exactly. Inspected the after PNG.
Built the locked release binary offline and installed it in `.venv/bin`; the
installed file matches the release SHA-256 and its `--help` command succeeds.
Existing running processes were not stopped or restarted.

Also reviewed [widget traits](https://ratatui.rs/concepts/widgets/),
[custom widgets](https://ratatui.rs/recipes/widgets/custom/),
[popup clearing](https://ratatui.rs/recipes/render/overwrite-regions/),
[debugging widget state](https://ratatui.rs/recipes/testing/debug-widget-state/),
and [0.30.2 release fixes](https://ratatui.rs/highlights/v0302/).
The battery correctly uses Buffer cells, modals clear their regions before
drawing, and diagnostics have a separate view. Cargo resolves one Crossterm
version (0.29.0) shared by direct input handling and Ratatui's backend. No widget
replacement, terminal backend change, or dependency upgrade is justified.

### 7. Prevent large Unix keyboard bursts from stalling

A native regression test sent 2,048 local Tab focus changes followed by Ctrl-Q
in one PTY write. The frontend failed to quit even while the test drained its
output. A daemon-free fixture using only Crossterm `read()` reproduced the
failure: it consumed exactly 1,024 events and then waited with input still
pending. This isolates the problem from the frontend's event budget and network
workers. Inspection of the installed Crossterm 0.29.0 Mio source found its
1,024-byte read buffer and return-before-draining behavior.

Enabled Crossterm's documented
[`use-dev-tty` feature](https://docs.rs/crate/crossterm/0.29.0/source/Cargo.toml.orig),
which selects raw file-descriptor polling on Unix. Both reproductions now consume
the complete burst and quit successfully. The production event loop, Ratatui
backend, and render cadence are unchanged. The feature adds `filedescriptor` and
its `thiserror` dependencies to both lockfiles; resolution/builds used the offline
crate cache. No dependency fork or custom input parser was introduced.

Also reviewed Tokio's
[`watch::Receiver::changed`](https://docs.rs/tokio/latest/tokio/sync/watch/struct.Receiver.html)
and [`select!` cancellation safety](https://docs.rs/tokio/latest/tokio/macro.select.html).
Two new tests confirm that log navigation cancels an HTTP request whose response
has stalled, processes the latest queued navigation, and exits if the request
channel closes during an in-flight query. This behavior already works correctly;
the log worker needed no implementation change.

Validation: all 34 Rust tests, 9 xtask tests, and 20 native PTY cases passed.
The full PTY suite covers resize, controls, mouse/modal behavior, bracketed paste,
TLS transport, input bursts, signals, and terminal restoration against isolated
simulated daemons. Formatting, Ruff, Clippy with warnings denied, and diff checks
passed. All 15 SVGs pass `--check`; regenerated SVG hashes and Logs PNG bytes
match the before set exactly. Inspected the after PNG.

### 8. Keep complete Unicode graphemes when truncating feedback

Reviewed Ratatui's [text recipe](https://ratatui.rs/recipes/render/display-text/)
and [`Span::styled_graphemes`](https://docs.rs/ratatui/0.30.2/ratatui/text/struct.Span.html#method.styled_graphemes),
then compared them with status-line truncation. The existing code walked Unicode
scalar values and could split a displayed grapheme: a narrow status region turned
`🇵🇱 connection restored` into `🇵…`. A TestBackend regression reproduced this
before the fix.

Truncation now walks Ratatui's own grapheme iterator and measures each complete
grapheme with `Span::width`, reserving the existing ellipsis column. No new
dependency or layout implementation was added. Fitting messages borrow their
original text; truncated messages no longer clone and remeasure the growing
prefix for each scalar value. These remove unnecessary allocations, without a
claim about overall terminal throughput.

Validation: 35 Rust tests and 9 xtask tests passed, including flag, keycap, ZWJ
emoji, combining-accent, double-width, exact-fit, one-column, and zero-column
status cases. The native PTY status-confirmation/fading case passed against an
isolated simulated daemon. Formatting, Clippy with warnings denied, and diff
checks passed. All 15 SVGs pass `--check`; regenerated SVG hashes and command-pending
dashboard PNG bytes match the before set exactly. Inspected before and after PNGs.

Finished the remaining Counter
[error-handling tutorial](https://ratatui.rs/tutorials/counter-app/error-handling/)
sections, and reviewed JSON Editor's
[editing](https://ratatui.rs/tutorials/json-editor/ui-editing/),
[exit](https://ratatui.rs/tutorials/json-editor/ui-exit/), and
[closing thoughts](https://ratatui.rs/tutorials/json-editor/closing-thoughts/) details.
The production guard restores the terminal before reporting a fatal error;
recoverable daemon failures stay in the status strip. Modal rendering already
uses `Clear` within its bounded region. Adding an error-reporting framework or
copying the tutorial's screen layout is unnecessary.

### 9. Exclude already loaded archive rows from the new-log counter

Compared the frontend archive/stream merge with the daemon's actual WebSocket
behavior: each connection sends retained records after its snapshot. A regression
loaded four rows through an archive page and then replayed those same identities
through the stream. The viewport stayed fixed, but `unseen` incorrectly became
four even though the rows were already in the loaded archive.

The archive path now checks the existing loaded identities before increasing
`unseen`, using the same `(server_instance_id, sequence)` comparison as live
insertion. The bounded recent stream cache remains populated, and repeated stream
events still return false. An unseen row is counted once; the same sequence from
another daemon instance remains a distinct row. No cache index, extra retained
state, or layout change was added.

Validation: all 36 Rust tests passed. The new regression covers archive replay,
genuinely new records, repeated stream events, daemon-instance identity, unchanged
scroll offset, and unchanged archive clipboard contents. The native PTY
day/archive/lazy-page/drag/live-resume test passed against an isolated simulated
daemon. Formatting, Clippy with warnings denied, and diff checks passed. All 15
SVGs pass `--check`; regenerated SVG hashes and Logs PNG bytes match the before
set exactly. Inspected the after PNG.

Reviewed the complete [Async GitHub example](https://ratatui.rs/examples/apps/async-github/)
and [Scrollbar demo](https://ratatui.rs/examples/apps/scrollbar/), plus the installed
0.30.2 scrollbar implementation. The app already separates network work from
rendering and clips the log viewport before constructing Paragraph rows. Its
scrollbar state correctly uses `max_offset + 1` with the actual viewport length:
Ratatui's thumb calculation adds that viewport to the maximum position. Using
the total row count in its place would count the viewport twice. No replacement
for the current pagination or scrollbar is warranted.

### 10. Verify clipboard timeout and quit while copying

Reviewed the [Ratatui panic example](https://ratatui.rs/examples/apps/panic/),
Tokio's [bounded runtime shutdown](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#method.shutdown_timeout),
and [`Command::kill_on_drop`](https://docs.rs/tokio/latest/tokio/process/struct.Command.html#method.kill_on_drop).
The current frontend restores its terminal before runtime shutdown and bounds
that shutdown to 200 ms. Clipboard work uses asynchronous pipes, a two-second
timeout, and a kill-on-drop child; it does not write to the terminal from the
worker.

Added two native PTY regressions with a fake `wl-copy` executable that stalls for
30 seconds. On timeout, the UI reports copy failure and remains running; on
Ctrl-Q, it exits within the 1.5-second test bound without waiting for that timeout.
Both verify terminal restoration and that the helper has exited. The test uses
a Linux pidfd to observe and clean up only its own helper, avoiding PID reuse
races. It never invokes the real desktop clipboard.

Both cases passed with the existing production code, so no implementation change
was justified. The tests verify child termination, not an additional guarantee
about when the operating system reaps it; Tokio documents reaping as best effort.
Ruff checks/formatting and diff checks passed. All 15 SVGs pass `--check`;
regenerated SVG hashes and live dashboard PNG bytes match the before set exactly.
Inspected the after PNG. Existing PTY coverage separately verifies quitting
during an output operation leaves the daemon and its station state independent
of the client.

### 11. Exercise repeated resize and collapsed terminal areas

Reviewed [buffer/diff rendering](https://ratatui.rs/concepts/rendering/under-the-hood/),
[dynamic layouts](https://ratatui.rs/recipes/layout/dynamic/), and the
[FAQ's buffer-bound guidance](https://ratatui.rs/faq/#how-do-i-avoid-panics-due-to-out-of-range-calls-on-the-buffer).
The renderer obtains its area from `Frame`, paints a complete frame, and returns
through the minimum-size path before computing dashboard/modal offsets. Its
private battery widget is only passed in-bounds layout regions.

Existing layout tests rendered separate fresh terminals. Added a regression that
reuses one Terminal while repeatedly resizing its TestBackend and calling the
production resize handler. It covers 550 combinations: all five views, 11 widths
from zero through 120, and 10 heights from zero through 40. These include 0x0,
single-cell areas, minimum-size boundaries, and the dashboard's maximum-size
boundaries. Each frame's cells/styles must equal a fresh render, preventing stale
content from surviving shrink/expand transitions. Every active hitbox must fit
the frame; all dashboard/modal hitboxes must clear below the minimum size. A
fixed clock keeps freshness, animation and feedback consistent throughout.

The regression passed without changing production rendering or adding defensive
layout wrappers. All 37 Rust tests, formatting, Clippy with warnings denied, and
diff checks passed. All 15 SVGs pass `--check`; regenerated SVG hashes and the
too-small-terminal PNG bytes match the before set exactly. Inspected the after PNG.

### 12. Verify history expiration and timeline identity

Compared the [Sparkline example](https://ratatui.rs/examples/widgets/sparkline/)
with production history projection. The application already passes explicit
100 W / 300 W maxima, uses two rows, and tracks history by timestamp rather than
event count. The idle predicate checks positive samples independently of pixel
quantization, so small or overwritten readings still delay the idle animation.

Added two fixed-clock regressions. The first checks that a sample exactly 120
seconds old remains visible and blocks idle, then expires from both the graph
and idle predicate one millisecond later. It also covers zero-width graph data
and pruning. The second checks duplicate suppression, clearing the timeline on
a backwards clock jump, daemon restarts that reuse sequence/segment identifiers,
and an explicit gap between station segments. Both passed with existing
production code; no graph or cache implementation change was warranted.

All 39 Rust tests, formatting, Clippy with warnings denied, and diff checks
passed. All 15 SVGs pass `--check`; regenerated SVG hashes and idle dashboard
PNG bytes match the before set exactly. Inspected the idle PNG before and after.

### 13. Verify responsiveness during sustained stream bursts

Reviewed [event handling](https://ratatui.rs/concepts/event-handling/), the complete
[terminal/event-handler recipe](https://ratatui.rs/recipes/apps/terminal-and-event-handler/),
and Tokio's [bounded channels](https://docs.rs/tokio/latest/tokio/sync/mpsc/fn.channel.html).
MyPowers already separates blocking terminal polling from asynchronous network
workers. It processes at most 256 incoming events before polling terminal input;
the shared queue has capacity 256 and applies backpressure. The recipe's
unbounded event queue is an example, not a reason to replace this bounded design.

Added two native PTY stress regressions using a local WebSocket server and a
status template from the isolated simulated daemon. The server sends an initial
2,048-log burst, then sustained log traffic and fresh status updates. UI-visible
markers prove the frontend consumed at least 4,096 log events. While streams
continue, Help opens and terminal resizing reaches both the minimum-size warning
and the restored Help view. Ctrl-Q and SIGTERM each exit within the two-second
test bound and restore terminal attributes. No output command is sent.

The initial test incorrectly required both producers' send counters to keep
advancing; socket backpressure can correctly stall the log producer. Replaced
that assertion with a marker proving actual UI consumption. Both cases then
passed with unchanged production code. Ruff checks/formatting and diff checks
passed. All 15 SVGs pass `--check`; regenerated SVG hashes and Help PNG bytes
match the before set exactly. Inspected Help before and after.

## Remaining review

- Review remaining applicable application examples and tutorial integration details.
- Further task lifetime/cancellation checks; terminal cleanup and signals now have PTY coverage.
- Key/mouse routing, resize handling, input bursts, and operation responsiveness.
- Production widgets: Block, Paragraph, Sparkline, Scrollbar, and custom battery Buffer writes.
- Unicode widths, control-character sanitation, and status-line truncation.
- History retention, archived log pagination, cancellation, and live stream validation.
- Layout bounds, cached layout work, render allocation cost, and measurement before optimization.
- Feature flags, version-specific APIs, and testing practices relevant to this frontend.

Each modification cycle must have before images, targeted tests, after images,
an exact comparison proving visual preservation, and its own commit. Review-only
findings do not justify speculative abstractions or cosmetic changes.
