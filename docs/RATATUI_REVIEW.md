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

### 14. Borrow sanitized display text instead of allocating every frame

Reviewed [text primitives](https://ratatui.rs/recipes/render/display-text/),
[Paragraph rendering](https://ratatui.rs/recipes/widgets/paragraph/), and the
installed Span API. Ratatui accepts borrowed `Cow<str>` content; the sanitizer
previously allocated a fresh String for every display value, even when it had
no controls and already fit the 2,000-scalar limit.

The sanitizer now returns borrowed text for that common case and allocates only
when filtering or truncation is needed. Feedback and the Settings row builder
explicitly own values that must outlive their source strings. Control removal
and the scalar limit remain unchanged. No cache, additional dependency, widget
replacement, or layout change was added.

Extended the offline rendering benchmark with a System allocator wrapper that
counts allocation/reallocation calls in 100 warmed frames. Counting is disabled
during the five timing rounds; the wrapper itself is benchmark-only. These are
whole TestBackend-frame counts, not peak memory measurements or daemon metrics.

| Scene | Calls/frame before → after | Median µs/frame before → after |
| --- | --- | --- |
| Dashboard live | 94 → 90 | 130.35 → 126.24 |
| Dashboard idle | 98 → 94 | 125.93 → 121.93 |
| Logs modal | 378 → 294 | 266.45 → 260.48 |

The log frame makes 84 fewer allocation/reallocation calls (about 22%). Timing
differences are small; overlapping ranges do not support a strong claim about
end-to-end speed. The renderer's refresh interval remains 250 ms.

All 40 Rust tests, 9 xtask tests and 24 native PTY cases passed, including
live/archive logs, simulated controls, stream/input bursts, copying, TLS,
signals, panic cleanup and terminal restoration. A new sanitizer regression
covers borrowed UTF-8 input, C0/C1 controls, terminal escape sequences, and
2,000-scalar boundaries with multibyte characters. Formatting, Clippy with
warnings denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated
SVG hashes and Logs PNG bytes match the before set exactly. Inspected Logs
before and after. Built the release binary and updated the local installed
frontend; existing processes were not restarted.

### 15. Validate timestamps in every WebSocket envelope

Reviewed the [Elm architecture guidance](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/)
and compared frontend stream handling with the [API contract](api.md#stream-schema)
and daemon emitter. Separating network effects from state updates remains
appropriate; mutable rendering is limited to viewport/hitbox state that depends
on the actual Frame area. No component framework or additional event layer is
needed.

Found that the Rust stream envelope omitted the required `server_time` field.
Consequently, a heartbeat with missing or invalid time was considered a valid
message and refreshed the connection's receive deadline. A local WebSocket
regression failed with the existing decoder, proving the missing validation.

Added the required String field and RFC3339 validation before processing any
message or refreshing the deadline. Invalid, null and absent heartbeat times
now close the stream before the following state can reach the UI. The regression
also checks that a valid heartbeat permits that state. Existing mock envelopes
now include the required timestamp; the production daemon already emitted it.
No compatibility path, wall-clock skew rule, or visual change was added.

All 41 Rust tests and 24 native PTY cases passed. Formatting, Ruff, Clippy with
warnings denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated
SVG hashes and daemon-offline PNG bytes match the before set exactly. Inspected
the offline PNG before and after. The release build and local installed frontend
were updated without restarting existing processes.

### 16. Verify the battery widget's Buffer and fill boundaries

Reviewed [custom widgets](https://ratatui.rs/recipes/widgets/custom/),
[widget selection guidance](https://ratatui.rs/concepts/widgets/), the complete
[Gauge example](https://ratatui.rs/examples/widgets/gauge/), and the installed
0.30.2 Gauge implementation. The small custom battery Widget remains justified:
the built-in gauge styles do not provide this per-cell RGB gradient. Its writes
stay inside Ratatui's Buffer and the production renderer supplies a one-row,
in-bounds area. No raw ANSI output or new widget abstraction is needed.

Added two focused regressions. Across seven widths and all 101 valid percentages
(707 combinations), filled coverage must grow monotonically, have no holes, and
stay within a quarter cell of the requested proportion. Non-zero Buffer origins
and sentinel cells verify that neighboring rows/columns remain untouched. Empty
areas are no-ops. Separate cases verify the five gradient stops, clamping above
100%, and monochrome full/half/unfilled symbols. These test rendering invariants
without reproducing the widget's integer rounding formula.

All 43 Rust tests, formatting, Clippy with warnings denied, and diff checks passed.
No production change was needed. All 15 SVGs pass `--check`; regenerated SVG hashes
and live dashboard PNG bytes match the before set exactly. Inspected the live
PNG before and after. The installed binary from cycle 15 remains current because
this cycle changes only tests and documentation.

### 17. Verify heartbeat liveness and telemetry expiration

Reviewed the remaining [component event-loop and teardown template](https://ratatui.rs/templates/component/tui-rs/),
the [JSON editor's finished UI](https://ratatui.rs/tutorials/json-editor/closing-thoughts/),
and the installed [Tungstenite WebSocket contract](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocket.html).
The frontend keeps one synchronous terminal reader and bounds network work per
iteration; existing native burst tests cover responsiveness. Adopting the
template's unbounded event channel or additional cancellation layer would not
improve this application. Pong frames echo Ping payloads without replacing
application-level receive deadlines.

Added an isolated WebSocket regression using the production stream consumer and
App updates. Valid heartbeats arrive every five seconds through second 20,
while Ping frames continue each second. The API remains reachable after station
telemetry expires, but output commands become unavailable. After the final
heartbeat, the stream times out after 15 seconds despite continuing Pings.
The server checks matching Pong payloads in send order; no heartbeat or Ping
becomes a telemetry update. This uses the real timeout without extra Tokio
features, daemon access, or hardware. The existing implementation passed;
no production change was warranted.

All 44 Rust tests passed in 35.12 seconds. Formatting, Clippy with warnings
denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated SVGs
and reconnecting PNG bytes match the before set exactly. Inspected the
reconnecting PNG before and after. The installed frontend remains current
because this cycle changes only tests and documentation.

### 18. Bound live log growth during failed page refreshes

Reviewed [text display](https://ratatui.rs/recipes/render/display-text/),
[styles](https://ratatui.rs/recipes/render/style-text/), and
[widget-state debugging](https://ratatui.rs/recipes/testing/debug-widget-state/).
The production renderer and SVG exporter already handle terminal-cell widths
and styles; existing Unicode regressions cover their use. No alternate text
layout or visual change was needed. Following the data path beyond rendering
exposed a cache issue that bounded network channels alone cannot prevent.

When HTTP page refreshes failed repeatedly but the live log stream kept working,
each failure cleared the loading flag, allowing more rows to accumulate before
the next refresh. A regression reproducing this condition exceeded the intended
live cache budget after the seventh 200-record batch. The archive page-count
limit did not bound records appended to a live page.

Added an append limit of five page sizes plus the existing 1,000-record catch-up
allowance. The separate recent-stream buffer remains limited to 1,000 records.
Loaded rows and cursor metadata stay intact; automatic latest-page requests
continue, and a successful response restores the current range and merges
arrivals received during that request. Named the existing page/recent limits so
their uses agree. This bounds retained record counts, not a measured RSS budget;
HTTP response and WebSocket message size limits remain separate protections.

The recovery regression also proved that a row received during the HTTP request
was shown but still counted as unseen. Successful live catch-up now clears that
count. Archive mode continues counting arrivals without moving its viewport.
The regression checks 3,000 arrivals with repeated failures, clipboard stability,
bounded retention, live recovery with an in-flight arrival, resumed appends,
and the server-provided cursor for older records.

All 45 Rust tests and 24 native PTY cases passed (35.10 and 83.91 seconds).
Formatting, Clippy with warnings denied, and diff checks passed. All 15 SVGs
pass `--check`; regenerated SVGs and Logs PNG bytes match the before set exactly.
Inspected Logs before and after. Built the release frontend and updated the local
installed binary without restarting existing processes.

### 19. Verify stream rejection and fragmented-message bounds

Compared the [async application example](https://ratatui.rs/examples/apps/async-github/)
with the frontend's channel-driven network updates and read the installed
[WebSocket configuration contract](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocketConfig.html).
The renderer never waits on a network lock. Existing 16-KiB frame/message limits
are explicit and smaller than the library defaults; the aggregate message limit
also applies to fragmented messages. No dependency or architecture change was
needed.

Added regressions that exercise the production reconnecting stream worker,
its queued events, and App control gating against isolated loopback servers.
Eleven malformed envelope/status cases, a missing initial snapshot and binary
JSON must disconnect before a later valid state can reach the UI. Cases cover
non-increasing sequences, schema/instance mismatches, out-of-range telemetry,
missing live samples and unsupported event kinds. A positive sequence-gap case
preserves the API contract's increasing rather than consecutive sequence rule.
Every disconnect disables output controls; no command request is produced.

A second regression sends valid JSON messages at exactly 16,384 bytes and one
byte above that limit, both as single frames and as two smaller fragments.
At the limit the state reaches the UI; above it the worker disconnects first.
This verifies aggregate-message enforcement rather than only inspecting the
configured frame limit. Both regressions passed the existing implementation.

All 47 Rust tests passed in 35.10 seconds. Formatting, Clippy with warnings
denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated SVGs and
daemon-offline PNG bytes match the before set exactly. Inspected that scene
before and after. No native PTY rerun or installed-binary update was needed:
this cycle changes only tests/documentation, and cycle 18's 24 native cases and
installed release binary still cover the current production implementation.

### 20. Reject malformed startup bytes without panic or disclosure

Reviewed [CLI arguments](https://ratatui.rs/recipes/apps/cli-arguments/) and
[configuration directories](https://ratatui.rs/recipes/apps/config-directories/).
The existing small parser needs no additional argument/configuration framework.
The user-required current-directory `.env` contract takes precedence over the
recipe's XDG example; configuration locations and precedence remain unchanged.

Rust's [string environment iterator](https://doc.rust-lang.org/std/env/fn.vars.html)
and [string argument iterator](https://doc.rust-lang.org/std/env/fn.args.html)
panic on non-Unicode input. A subprocess reproduction on the installed toolchain
proved that an unrelated byte-valued environment entry, a malformed MyPowers
setting and a malformed argument each exited with code 101 and echoed the fake
input in the panic. Four regression cases also cover a non-Unicode environment
key; all failed before the fix.

Configuration now iterates OS strings, ignores unrelated environment entries,
and converts MyPowers values/CLI arguments with explicit UTF-8 validation.
Invalid inputs return concise errors without their values, before entering raw
mode or starting network tasks. No process-environment mutation, lossy decoding,
new dependency, configuration fallback or token/URL-validation change was added.
The tests use only fake markers in a minimal environment and an empty temporary
working directory. They check exit code 2, expected error text, no panic/input
echo, and no alternate-screen entry.

All 47 Rust tests and 28 terminal-suite cases passed (35.13 and 84.46 seconds).
Formatting, Ruff, Clippy with warnings denied, and diff checks passed. All 15 SVGs
pass `--check`; regenerated SVGs and Settings PNG bytes match the before set
exactly. Inspected Settings before and after. Built the release frontend and
updated the local installed binary without restarting existing processes.

### 21. Verify total HTTP deadlines for partial command responses

Reviewed the installed [Reqwest 0.12.28 timeout contract](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.timeout)
and its response-body implementation. The configured timeout is a total deadline
from connection through body completion; a per-read timeout would instead reset
after each successful read. The existing client already uses the total timeout
and explicitly disables request retries. No additional timeout layer is needed.

Added a regression using the production output-intention builder, command
performer, generic HTTP reader and App completion update. An isolated server
receives the captured AC PUT/revision/idempotency key, sends HTTP 202 and starts
a chunked JSON body, then continues supplying whitespace every 250 ms without
finishing the JSON. Despite continuing reads, the client returns after its
two-second total deadline with warning feedback that the outcome is uncertain.
No partial response becomes a command event and no optimistic AC state change
occurs. The next connection must be the caller's explicit GET, which succeeds;
a replayed PUT or additional request fails the test. The targeted regression
passed in 2.26 seconds without changing production code.

All 48 Rust tests passed in 35.12 seconds. Formatting, Clippy with warnings
denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated SVGs and
command-pending PNG bytes match the before set exactly. Inspected that scene
before and after. This cycle changes only tests/documentation, so cycle 20's
28 terminal-suite cases and installed release binary remain current.

### 22. Discard log pagination when its range changes

Checked the local log API's signed-cursor contract and the frontend's failed-page
handling. Cursors retain the day boundaries and minimum severity; the server
rejects a cursor paired with another range. Changing the severity cleared the
rows but retained old cursors, edge flags and scroll offset. If the replacement
request failed, scrolling could submit the previous filter's cursor. Returning
to today or crossing midnight also retained pagination from the previous day.
The new range regression failed before the fix.

Added one private cache-clear operation for the existing four range-changing
paths: filter changes, day navigation, End from another day, and live midnight
rollover. It clears rows, cached page boundaries, cursors, edge flags and offset
together. The recent live catch-up buffer and request-generation checks remain
unchanged. Home, End within today, and page-size changes keep valid loaded rows
and cursors when their refresh fails. Two regressions cover all four transitions
and these three same-range failure cases; subsequent scrolling cannot carry a
cursor from the discarded range.

All 50 Rust tests passed in 35.11 seconds; the final fixture refinements also
passed their targeted tests. Formatting, Clippy with warnings denied, and diff
checks passed. All 15 SVGs pass `--check`; regenerated SVGs and Logs PNG bytes
match the before set exactly. Inspected Logs before and after.
All 28 terminal-suite cases passed in 84.72 seconds against isolated simulated
services. Built the release frontend and updated the local installed binary;
existing application processes were not restarted.

### 23. Verify cleanup after a production render-output failure

Reviewed the [advanced widget implementation example](https://ratatui.rs/examples/apps/advanced-widget-impl/)
and [tracing recipe](https://ratatui.rs/recipes/apps/log-with-tracing/). The battery
widget holds only cheap, ephemeral draw inputs, so consuming it during rendering
is appropriate; there is no retained widget state to clone or move into a boxed
widget. The renderer already updates mouse hitboxes from its actual layout. The
logging recipe avoids direct terminal writes by using a file writer. MyPowers
already obtains application logs from its daemon and reports frontend errors
after cleanup, so adding a second logger or log destination is not justified.

The existing guard tests cover normal exit, main/worker/task panics, setup-write
failure, and input bursts. Added a test for the production main loop losing its
output after successfully rendering a frame. It runs the actual client with two
PTYs: an independently inspectable controlling input terminal and a separate
output terminal. Both network workers connect only to an isolated, silent local
listener. After the MYPOWERS frame appears and raw input mode is confirmed, the
test closes the output PTY master and sends Tab to the input PTY. The client exits
with code 2, restores the original input termios exactly, and reports the existing
I/O cleanup diagnostic without panicking. No production change was necessary.

The new regression and six existing guard tests passed in 0.73 seconds. Ruff
format/check and diff checks passed. All 15 SVGs pass `--check`; regenerated SVGs
and daemon-offline PNG bytes match the before set exactly. Inspected that scene
before and after. Cycle 22's full Rust/native suite and installed release binary
remain current because this cycle only adds a test and documentation.

### 24. Verify the complete HTTP body-size boundary

Reviewed archive/stream ordering against the existing Logs documentation. The
footer reports new stream arrivals while the archive stays stationary; End
fetches today's latest range and enables follow. That is different from an
unread counter tied to the visible viewport. No new acknowledgement model or
counter state was introduced during this internal review.

Checked the installed Reqwest 0.12.28 `Response::chunk` implementation in
`src/async_impl/response.rs`: it yields successive data frames, not one complete
response. The existing MyPowers reader accumulates bytes and checks the combined
size before extending its buffer. Added a loopback-server regression for the
16-MiB boundary with both Content-Length and HTTP chunked framing. Each response
contains valid JSON plus whitespace, sent in 64-KiB writes. Exactly 16 MiB must
return the JSON object; one byte more must return the client-limit error in both
framing modes. This verifies an aggregate limit rather than a per-chunk check.
The exact-boundary server must finish normally; an oversized response may be
interrupted by the client's rejection. Production code already behaves correctly.

The four boundary cases passed in 0.70 seconds; all 51 Rust tests passed in
35.10 seconds. Formatting, Clippy with warnings denied, and diff checks passed.
All 15 SVGs pass `--check`; regenerated SVGs and Logs PNG bytes match the before
set exactly. Inspected Logs before and after. Production is unchanged, so the
installed release binary and previous terminal verification remain applicable.

### 25. Require a textual log level before storing or copying records

Completed the Counter App multi-file tutorial's
[main loop](https://ratatui.rs/tutorials/counter-app/_multiple-files/main/),
[terminal wrapper](https://ratatui.rs/tutorials/counter-app/_multiple-files/tui/),
and [update function](https://ratatui.rs/tutorials/counter-app/_multiple-files/update/).
MyPowers already separates rendering, state updates and terminal ownership, filters
key event kinds at the input boundary, and limits drawing to its 250-ms cadence.
Its existing terminal guard and context-specific exit keys meet the application's
requirements; the simpler tutorial wrapper does not justify replacing them.

While reviewing data entering the log buffer, found that the shared HTTP/stream
record validator required timestamps, identity, sequence and message, but omitted
the level's type. Missing levels could reach the archive and become fallback INFO
in copied text. Checked the production Diagnostics record builder: it always
writes a textual level. Added one shared validation condition requiring that
field to be a string. This preserves named logging levels instead of narrowing
the record schema to the four API filter choices.

The new HTTP page regression failed on a missing level before the fix; the
expanded WebSocket regression failed on a numeric level. The HTTP test covers
missing/null/numeric/array/object levels plus DEBUG, INFO, WARNING, ERROR and
CRITICAL names. Invalid pages/stream records are rejected before entering the UI;
correct rows retain their original values. All 14 log tests passed in 1.68 seconds
and the full 52-test Rust suite passed in 35.08 seconds. Formatting, Clippy with
warnings denied, and diff checks passed. All 15 SVGs pass `--check`; regenerated
SVGs and Logs PNG bytes match the before set exactly. Inspected Logs before and
after.
All 29 terminal-suite cases passed in 81.57 seconds against isolated services.
Built the release frontend and updated the local installed binary, without
restarting existing processes.

### 26. Review text/style representation and preview fidelity

Completed the [text recipe](https://ratatui.rs/recipes/render/display-text/),
[style recipe](https://ratatui.rs/recipes/render/style-text/), and
[modifier example](https://ratatui.rs/examples/style/modifiers/). Compared them
with production Span/Line/Paragraph use and the cell exporter. The renderer uses
the appropriate text primitives and delegates clipping/alignment to Ratatui.
The exporter paints all full-cell backgrounds before glyphs, uses each cell's
terminal width, and keeps combining clusters together. Existing tests exercise
CJK, combining marks, box/block glyphs, RGB backgrounds, ANSI/indexed colors,
reversed/hidden cells, bold and foreground dimming. Its fixed palette and font
declaration are preview assumptions, not a claim that every terminal has the
same palette or glyph outlines; these limits are already documented.

Also checked the installed Crossterm backend's modifier-diff implementation:
removing bold or dim resets intensity and reapplies whichever modifier remains.
The production UI does not need its own ANSI/style reset engine. No rendering or
export implementation change was warranted. All nine xtask tests passed in
0.40 seconds and xtask Clippy with warnings denied passed. Saved all 15 scenes
before this documentation update, verified them with `--check`, regenerated and
compared them exactly. The 80x24 live-dashboard PNG also remains byte-identical;
inspected it before and after. Production/test code and the installed binary
remain unchanged.

### 27. Keep Pong write backpressure inside the stream deadline

Reviewed [Tungstenite 0.30's automatic control replies](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocket.html)
and the installed Tokio-Tungstenite adapter. Receiving Ping queues a matching
Pong; a later flush sends it. MyPowers protected reads with its valid-message
deadline but awaited a separate manual Pong send without any deadline. A peer
that stopped reading replies could therefore prevent stream loss from being
reported indefinitely, even though telemetry freshness still expired normally.

Added a real loopback backpressure regression. The server has a small receive
buffer, sends one valid snapshot followed by a burst of valid 125-byte Ping
frames, and never reads the client's replies. Before the fix, the stream exceeded
the test's 16-second outer guard instead of honoring its 15-second deadline.
The server task is aborted and awaited before assertions so failure leaves no
detached producer. Replaced manual Pong construction with an automatic-reply
flush, bounded by the remaining time since the last valid API message. Neither
Ping nor Pong renews that clock. Fully flushing before reading another frame
also keeps write backpressure effective rather than accumulating control replies.

The targeted regression passed in 15.07 seconds. All 53 Rust tests passed in
35.11 seconds, including the existing exact-Pong-payload, heartbeat and telemetry
expiration checks. All 29 terminal-suite cases passed in 81.60 seconds. Formatting,
Clippy with warnings denied, and diff checks passed. All 15 SVGs pass `--check`;
regenerated SVGs and daemon-offline PNG bytes match the before set exactly.
Inspected that scene before and after. Built the release frontend and updated
the installed local binary without restarting existing application processes.

### 28. Preserve command identity through result polling

Completed the Counter tutorial's [application state](https://ratatui.rs/tutorials/counter-app/_multiple-files/app/)
and [UI rendering](https://ratatui.rs/tutorials/counter-app/_multiple-files/ui/)
sections. Production already separates state transitions, effects, and rendering;
the examples do not warrant another application framework or layout rewrite.
Reviewed the boundary between network results and those state transitions.
The server retains each command's output and requested value while updating its
status. The frontend checked those immutable fields on admission, but later GET
responses checked only the UUID and general DTO validity. A contradictory result
with the same UUID could therefore display confirmation of another intention.

A real loopback regression reproduced AC ON being reported as DC ON confirmed.
Result polling now requires both the original output and requested value, in
addition to its existing UUID validation. Contradictions return the existing
uncertain-outcome warning immediately and never retry the output operation.
The regression covers seven replies: valid confirmation, changed DC/Lamps output,
changed requested value, contradictory intermediate status, and another UUID.
It also checks that only the valid admission becomes a command event, pending
state clears, and feedback never replaces actual station telemetry.

The targeted regression passed in 3.93 seconds and all 54 Rust tests passed in
35.11 seconds. Formatting, Clippy with warnings denied, and diff checks passed.
All 15 SVGs pass `--check`; regenerated SVGs and command-pending PNG bytes match
the before set exactly. Inspected that scene before and after.
All 29 terminal-suite cases passed in 84.55 seconds against isolated services.
Built the release frontend and updated the installed local binary without
restarting existing application processes.

### 29. Verify terminal event kinds through the actual input parser

Completed the [User Input example](https://ratatui.rs/examples/apps/user_input/)
and checked the [keyboard protocol's event types and functional keys](https://sw.kovidgoyal.net/kitty/keyboard-protocol/).
The example routes input by application mode and accepts press events. MyPowers
already filters event kinds before contextual dispatch; no input editor or
additional framework is needed for its current read-only settings and shortcuts.
Extended coverage at the production process boundary rather than calling App's
key method directly and bypassing that filter.

The new PTY test sends press/repeat/release encodings through Crossterm, exercises
Alt/Control modifiers and F3, and uses ordinary help presses as ordered barriers.
It verifies that ignored input does not change outputs/revision, connection
session/desired state, logging level, or the active help context. Valid presses
still open Logs, confirm AC ON against the isolated simulated backend, and quit
immediately with Ctrl-Q while restoring the original terminal attributes.
The F3 encoding uses the protocol's tilde form; CSI R is reserved for cursor
position reports. MyPowers does not negotiate additional keyboard reporting
modes: this test covers supported events when received, not universal terminal
support or suppression of legacy auto-repeat that arrives as ordinary presses.

The new regression passed in 3.97 seconds. Python linting, formatting, and diff
checks passed. Production code and the installed release binary remain unchanged.
All six selected keyboard/modal/paste/resize/restoration cases passed in
28.55 seconds. All 15 SVGs pass `--check` and match the saved before set exactly;
the Logs Help PNG is also byte-identical and was inspected before and after.

### 30. Verify modal transitions and complete rendered frame state

Completed the [Popup example](https://ratatui.rs/examples/apps/popup/) and
[Flex example](https://ratatui.rs/examples/layout/flex/). Production already uses
Clear before modal contents, Block's inner area, and explicit centered Layout
constraints. Its fixed dashboard/modal bounds do not warrant a layout rewrite.
The Flex demo enlarges its layout cache for its many configurable constraint
combinations; MyPowers has fixed layouts and already enables caching. No cache
size expansion is justified by the existing measurements.

Added a deterministic transition regression using the actual production renderer.
It visits all 25 ordered pairs among Dashboard, Logs, Help, Settings, and Quit,
at 60x19, 80x24, 94x29, and 120x40, with both color modes and ASCII/Unicode data.
Across 800 reused-terminal frames, every complete pre-flush buffer matches a fresh
render exactly, including all symbols, backgrounds, foregrounds, and modifiers.
Fixtures alternate long/short text, filled/empty log lists, battery values,
connected/offline state, help context, and status feedback/counters. ASCII cases
also compare the complete post-flush TestBackend buffers.

The initial naive post-flush comparison exposed a TestBackend representation
limit rather than a production frame difference. Its draw method copies emitted
cells only; it does not emulate a terminal erasing the trailing half of a wide
glyph. Reused backing arrays can therefore retain hidden text/attributes, which
may later appear in that array although a physical terminal has erased them.
The installed Ratatui core 0.1.2 implementation confirms this behavior. Unicode
cases assert the full pre-flush frame without normalizing away any renderer data.
The canonical image generator already starts each scene with a fresh TestBackend,
so it does not accumulate this limitation. No renderer/backend workaround was added.

The new regression passed in 3.71 seconds; the full 55-test Rust suite passed in
35.07 seconds. Formatting, Clippy with warnings denied, and diff checks passed.
All 15 SVGs pass `--check` and match the saved before set exactly. The Settings PNG
is also byte-identical and was inspected before and after. Production code and
the installed release binary remain unchanged.

### 31. Measure and apply release optimization for renderer execution speed

Completed the [release recipe](https://ratatui.rs/recipes/apps/release-your-app/),
rechecked [backend compatibility](https://ratatui.rs/concepts/backends/), and
verified [Cargo's profile inheritance and overrides](https://doc.rust-lang.org/cargo/reference/profiles.html).
Both dependency graphs resolve one Crossterm 0.29, lockfiles are tracked, and
installation uses `--locked`. The native launcher preserves the current working
directory and forwards configuration flags to the Rust executable; both existing
dispatch/install-hint tests passed in 0.06 seconds. Release already uses thin LTO,
one codegen unit, and stripped symbols. Compared its size-oriented optimization
with Rust's default speed-oriented release optimization instead of assuming the
compiler setting was beneficial for this workload.

Ran the unchanged production-renderer benchmark sequentially with `opt-level=s`
and `opt-level=3`: five rounds of 5,000 frames per scene, with warmup and the same
deterministic TestBackend fixtures. The speed build used an isolated target under
`/tmp`; no daemon, hardware, real terminal, or second layout was involved.

| Scene | `s`, median us/frame (min–max) | `3`, median us/frame (min–max) |
| --- | ---: | ---: |
| Live dashboard | 126.96 (119.59–133.24) | 66.61 (65.61–67.36) |
| Idle dashboard | 126.50 (119.45–136.62) | 65.41 (63.43–67.03) |
| Logs modal | 259.90 (246.01–270.52) | 139.84 (133.52–145.23) |

Allocation/reallocation calls remain 90/94/294 per frame respectively. Release
size increases from 5,689,152 to 6,403,432 bytes (approximately 5.43 to 6.11 MiB,
12.6%). Removed `opt-level="s"` so the ordinary release profile now uses Cargo's
default `3`, retaining its other settings. The measured render cost is roughly
halved for this benchmark; this is not a claim of twice-as-fast network, BLE,
terminal I/O, or overall application response time. No dependency or application
logic change was needed.

All 55 Rust tests passed under the new release profile in 35.03 seconds. All 30
terminal-suite cases passed against an immutable copy of the measured release
binary in 86.61 seconds, with isolated simulated services. Formatting, Clippy with
warnings denied, and diff checks passed. A release-built xtask using matching
optimization/LTO/codegen/strip settings verified all 15 SVG scenes, and the normal
developer generator also passed `--check`. Regenerated SVGs and live-dashboard PNG
bytes match the saved before set exactly; inspected that scene before and after.
The standard release build matches the terminal-tested binary byte-for-byte.
Updated the installed local frontend from that build without restarting existing
application processes.

### 32. Align state-conflict feedback with the actual server contract

Completed the remaining [Elm architecture example](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/).
The existing event/update/render separation fits the application; mutable rendering
state for hitboxes and scrollbar geometry is intentional. No immutable-model copy
or framework replacement is warranted.

Cross-checking output error feedback against the server found that the frontend
recognized three unused conflict aliases, while the actual server emits
`state_conflict` for an obsolete daemon instance, an obsolete output revision,
or an output revision change while waiting for fresh telemetry. Replaced the
aliases with the actual code. A conflict now explains that station state changed
and the user can try again, rather than giving a generic rejection message.

The regression first reproduced the incorrect message through a real isolated
HTTP 409 response. It covers both immediate admission rejection and an accepted
command subsequently failing with that reason during polling. Each case asserts
error severity, no replay or further polling after failure, only the appropriate
admission event, cleared pending state, unchanged station telemetry, and no raw
server details in feedback.

All 56 Rust tests passed in 35.07 seconds. Five selected native terminal cases
passed against the updated release build in 26.35 seconds, including multiple
clients sharing one simulated daemon and controls/logs/paste/resize/restoration.
Formatting, Clippy with warnings denied, and diff checks passed. All 15 SVGs pass
`--check` and match the saved before set exactly; the command-pending PNG is also
byte-identical and was inspected before and after. Updated the installed local
frontend without restarting existing application processes.

### 33. Verify daemon restart while a command outcome is being polled

Rechecked the [event-handling models](https://ratatui.rs/concepts/event-handling/),
and completed the [Block recipe](https://ratatui.rs/recipes/widgets/block/) and
[Block example](https://ratatui.rs/examples/widgets/block/). The production central
input loop with asynchronous message passing fits its independent status, logs,
and operation workers. Rounded borders, integrated Line titles, and Block inner
areas already use the documented primitives. No new loop or border renderer is
needed.

Added a regression for a daemon restart between command admission and completion.
An isolated HTTP server accepts one PUT, waits until the frontend is polling its
command ID, and then returns the actual `command_not_found` HTTP 404 contract.
Before releasing that response, the test delivers a new daemon instance/session,
revision, telemetry segment, and AC state to the production App.

The existing implementation passes: old history is cleared, another command stays
blocked until the pending operation finishes, a lost outcome produces uncertainty
feedback, and neither another poll nor a replayed PUT is sent. The complete new
snapshot remains unchanged by that feedback. A subsequent explicit user command
captures the new instance/revision and derives its intention from the new AC state.
No production change or speculative cancellation mechanism was required.

The new regression passed in 0.56 seconds. Formatting, Clippy with warnings denied,
and diff checks passed. All 15 SVGs pass `--check` and match the saved before set
exactly; the command-pending PNG is byte-identical and was inspected before and
after. Production code and the installed release binary remain unchanged.

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
