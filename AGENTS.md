# Deterministic UI previews

Use `cargo xtask ui-snapshots` as the default system for UI screenshots and visual
inspection. Run it from the repository root to generate SVGs in
`artifacts/ui/` from the production Ratatui renderer and a deterministic TestBackend.
This needs no daemon, BLE, HTTP, terminal session, or desktop access. The developer
note is `artifacts/ui/README.md`. Do not create a second HTML/CSS layout for previews.
For direct image inspection, optionally convert an SVG to a temporary PNG with
`rsvg-convert` if installed; never launch Chrome for conversion.

# Fallback terminal screenshots

Use the `terminal-screenshot` skill only when `cargo xtask ui-snapshots` is not
available in the project or environment. If the generator is available but fails,
diagnose and fix it rather than switching screenshot systems. Do not start a
terminal or Xvfb session for routine previews when the generator is available.

Use `scripts/mypowers-hidden-terminal-test.sh` to inspect the Rust/Ratatui TUI in a real terminal. It runs `cargo run --locked --manifest-path frontends/tui/Cargo.toml` from the repository root using Alacritty on a hidden Xvfb display `:99`. This preserves the current-directory `.env` behavior.

```bash
./scripts/mypowers-hidden-terminal-test.sh
# Optional screenshot destination:
./scripts/mypowers-hidden-terminal-test.sh /tmp/mypowers-preview.png
```

- Requires Bash, Cargo, Xvfb, xdpyinfo, Alacritty, xdotool, ImageMagick `import`, and pgrep. Do not install missing dependencies without authorization.
- The default screenshot is `/tmp/terminal-screenshot.png`. Inspect it directly with the image-reading tool.
- Keep the script's terminal session running during inspection. Press Enter to quit the application and clean up its terminal and Xvfb server.
- If `:99` is occupied, the script stops. Do not remove existing locks or terminate unrelated servers.
- Never switch the user's workspace, focus windows on their desktop, or launch a browser to generate a terminal preview.
- For this fallback, follow the `terminal-screenshot` skill when available. Use an isolated simulated daemon for control tests. Read-only visual checks may use the existing daemon; never send output changes to real hardware as a screenshot action.
