# TUI snapshots

From the repository root:

```sh
cargo xtask ui-snapshots
```

This generates and overwrites the SVG files beside this note. Both the interactive
TUI and these images call `mypowers_tui::ui::draw`: scenes supply fixed state to
Ratatui `TestBackend`, then the exporter reads its cells. There is no alternate
layout, HTML/CSS mockup, daemon, BLE, HTTP, terminal session, or clipboard access.
The command reports generated files only after every scene has rendered and all
files have been written successfully; errors return a nonzero exit status.

To verify the current renderer against the saved images without overwriting them:

```sh
cargo xtask ui-snapshots --check
```

The check fails if any scene differs or its saved SVG is missing, lists all affected
files, and never creates or modifies artifacts. For internal changes, generate and
save the images before editing, run the check afterwards, then regenerate and
compare the before/after images. Commit each batch only after tests and visual
comparison pass. A matching SVG is an exact comparison of exported symbols,
positions, colors, and modifiers, not merely a matching text layout.

The offline verification workflow runs exporter tests and this comparison on
every push and pull request. CI shares the production TUI's Cargo target directory
to reuse common dependency builds; the saved SVGs are never regenerated in CI.

Fixtures use UTC `2026-10-05T12:00:00Z`, battery 71%, fixed history arrays,
fixed telemetry age, fixed feedback age, and a fixed idle animation frame.
Normal scenes use 120x30 terminal cells with the production dashboard's 94x29
limit. Additional scenes cover 80x24, 60x19, contextual Help, quit confirmation,
and an undersized 50x14 terminal. Reconnecting/device-offline scenes deliberately
have no telemetry sample; daemon-offline retains explicitly unavailable readings.
`dashboard-low-load.svg` covers INPUT 35 W / OUTPUT 3 W. The production Sparkline
starts each channel at 0–100 W and doubles its maximum to fit visible averages
and current readings. Positive samples below its first quantization step receive
one visible eighth-cell tick, while zero stays empty. This visibility floor does
not change telemetry or historical averages.
History uses server-computed averages in fixed UTC buckets: 10 seconds, 30 seconds, 60 seconds
or one hour per bar (`t` on the dashboard). `dashboard-live-60s.svg` and
`dashboard-live-1h.svg` capture the additional selections. Fixtures supply fixed
averages and sample counts directly, without SQL or HTTP. Completed bars retain
their values/colors between redraws and move left together at a bucket boundary.
The current bucket can change as recorded samples arrive. Missing buckets stay
empty; actual zeros participate in averages. Live numeric labels stay independent
of graph history, and positive fractional averages remain visible.

`dashboard-chart.svg` shows the production seven-row shared Chart widget with
Braille line datasets: INPUT green and OUTPUT cyan, one common auto scale starting
at 100 W and doubling as needed. The additional chart scenes cover 80x24, 60x19,
recording gaps, idle, low load and daemon loss. The 60s and 1h chart scenes
exercise time-axis formats. Axes use the real Chart/Axis widgets: power labels
on the left, and timezone-aware bucket times below seven plot rows. The plot
width excludes the seven reserved vertical-axis columns. `g` switches the visualization
locally without writing settings. The full-width chart requests one aggregate
bucket per column, and missing buckets break lines instead of becoming zero.
The production compositor unions overlapping Braille patterns from two renders
of the same Chart widget; shared cells use a neutral color instead of losing
one series. `dashboard-chart-nearby.svg` covers INPUT 49/51 W and OUTPUT 28 W.

SVG is canonical. Each cell is 10x20 SVG units. Background rectangles cover every
cell without gaps. Text preserves Unicode, RGB/ANSI colors, bold, and foreground
dim opacity; wide symbols retain their terminal span. Reverse, hidden, italic,
underline, and strike-through styles are handled. Blink is captured as a static
frame. A fixed monospace font family is declared without bundling a font, so exact
glyph outlines can vary across machines/viewers. Cell-height glyph sizing and
clipped cell viewports keep borders continuous and prevent block glyph overhang;
ordinary text keeps its monospace spacing. Explicit glyph transforms work without
relying on viewer-specific `textLength`/`lengthAdjust` support.

Settings uses the production Tabs widget (dashboard `s`). `settings-modal.svg`
shows Preferences; `settings-charts.svg`, `settings-alerts.svg`,
`settings-notify.svg` and `settings-debug.svg` cover every other tab. Additional
60x19 Charts/Debug scenes verify the minimum size; `help-settings.svg` shows
contextual help. Charts fixtures include saved preferences. The value-list scenes
`settings-interval-picker.svg` and `settings-visualization-picker.svg` show the
production editors. Debug scenes include Retry, Pause/Resume and Debug buttons.
The actual TUI persists all preference fields through the daemon settings API; snapshot generation
uses only fake state. Battery alerts and connectors remain future features. Warning/error counts in these fixtures demonstrate the
production status strip's rendering; they do not implement server alert tracking.
Zero counters are never displayed.

For optional local image inspection, if `rsvg-convert` is already installed:

```sh
rsvg-convert artifacts/ui/dashboard-live.svg -o /tmp/mypowers-dashboard.png
```

PNG conversion is not required by the generator and adds no Cargo dependency.

```sh
cargo test --locked --manifest-path xtask/Cargo.toml
cargo clippy --locked --manifest-path xtask/Cargo.toml --all-targets -- -D warnings
```
