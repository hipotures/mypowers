# Terminal UI visual checks

Use `skrypty/mypowers-hidden-terminal-test.sh` to inspect the Ratatui prototype in a real terminal. It runs `cargo run` in `prototypes/ratatui-ui` using Alacritty on a hidden Xvfb display `:99`.

```bash
./skrypty/mypowers-hidden-terminal-test.sh
# Optional screenshot destination:
./skrypty/mypowers-hidden-terminal-test.sh /tmp/mypowers-preview.png
```

- Requires Bash, Cargo, Xvfb, xdpyinfo, Alacritty, xdotool, ImageMagick `import`, and pgrep. Do not install missing dependencies without authorization.
- The default screenshot is `/tmp/terminal-screenshot.png`. Inspect it directly with the image-reading tool.
- Keep the script's terminal session running during inspection. Press Enter to quit the application and clean up its terminal and Xvfb server.
- If `:99` is occupied, the script stops. Do not remove existing locks or terminate unrelated servers.
- Never switch the user's workspace, focus windows on their desktop, or launch a browser to generate a terminal preview.
- Follow the `terminal-screenshot` skill when available. This prototype uses mock data only; visual checks must not connect to the production daemon or hardware.
