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

Fixtures use UTC `2026-10-05T12:00:00Z`, battery 71%, fixed history arrays,
fixed telemetry age, fixed feedback age, and a fixed idle animation frame.
Normal scenes use 120x30 terminal cells with the production dashboard's 94x29
limit. Additional scenes cover 80x24, 60x19, contextual Help, quit confirmation,
and an undersized 50x14 terminal. Reconnecting/device-offline scenes deliberately
have no telemetry sample; daemon-offline retains explicitly unavailable readings.

SVG is canonical. Each cell is 10x20 SVG units. Background rectangles cover every
cell without gaps. Text preserves Unicode, RGB/ANSI colors, bold, and foreground
dim opacity; wide symbols retain their terminal span. Reverse, hidden, italic,
underline, and strike-through styles are handled. Blink is captured as a static
frame. A fixed monospace font family is declared without bundling a font, so exact
glyph outlines can vary across machines/viewers. Cell-height glyph sizing and
clipped cell viewports keep borders continuous and prevent block glyph overhang;
ordinary text keeps its monospace spacing. Explicit glyph transforms work without
relying on viewer-specific `textLength`/`lengthAdjust` support.

Settings is the actual read-only Preferences and Debug / Diagnostics modal
(dashboard `s`; `b` toggles runtime DEBUG). Mutable server settings and an alert
engine are separate work. Warning/error counts in these fixtures demonstrate the
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
