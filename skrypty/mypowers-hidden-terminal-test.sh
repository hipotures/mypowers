#!/usr/bin/env bash
# Run from any directory. Press Enter after inspecting the screenshot to finish.
# Usage: ./skrypty/mypowers-hidden-terminal-test.sh [output.png]
set -euo pipefail

for command in Xvfb xdpyinfo alacritty xdotool import cargo pgrep; do
  command -v "$command" >/dev/null || {
    echo "Missing dependency: $command" >&2
    exit 1
  }
done

output=${1:-/tmp/terminal-screenshot.png}
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../prototypes/ratatui-ui" && pwd)
terminal_pid=''
xvfb_pid=''
app_pid=''
win=''

cleanup() {
  trap '' INT TERM
  # These are direct children of this shell; never terminate another terminal.
  if [[ -n "$terminal_pid" ]] && kill -0 "$terminal_pid" 2>/dev/null; then
    if [[ -n "$win" && -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
      DISPLAY=:99 xdotool key --window "$win" q >/dev/null 2>&1 || true
      for ((i = 0; i < 20; i++)); do
        kill -0 "$app_pid" 2>/dev/null || break
        sleep 0.05
      done
      if kill -0 "$app_pid" 2>/dev/null; then
        kill -TERM "$app_pid" 2>/dev/null || true
      fi
    fi
    kill -TERM "$terminal_pid" 2>/dev/null || true
    wait "$terminal_pid" 2>/dev/null || true
  fi
  if [[ -n "$xvfb_pid" ]] && kill -0 "$xvfb_pid" 2>/dev/null; then
    kill -TERM "$xvfb_pid" 2>/dev/null || true
    wait "$xvfb_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if xdpyinfo -display :99 >/dev/null 2>&1 \
  || [[ -e /tmp/.X99-lock || -e /tmp/.X11-unix/X99 ]] \
  || pgrep -f '^([^ ]*/)?(Xvfb|Xorg|Xwayland) :99( |$)' >/dev/null; then
  echo 'Display :99 is occupied. Leave the existing server and its locks untouched.' >&2
  exit 1
fi

Xvfb :99 -screen 0 1400x1100x24 -nolisten tcp >/tmp/terminal-screenshot-xvfb.log 2>&1 &
xvfb_pid=$!
printf '%s\n' "$xvfb_pid" >/tmp/terminal-screenshot-xvfb.pid
for ((i = 0; i < 100; i++)); do
  xdpyinfo -display :99 >/dev/null 2>&1 && break
  kill -0 "$xvfb_pid" 2>/dev/null || break
  sleep 0.05
done
if ! kill -0 "$xvfb_pid" 2>/dev/null || ! xdpyinfo -display :99 >/dev/null 2>&1; then
  cat /tmp/terminal-screenshot-xvfb.log >&2
  exit 1
fi

env -u NO_COLOR -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 DISPLAY=:99 \
  alacritty --class codex-terminal-screenshot --title codex-terminal-screenshot --hold \
  -o 'window.dynamic_title=false' 'window.dimensions.columns=94' 'window.dimensions.lines=24' 'font.size=14' \
  --working-directory "$project_dir" -e bash -c 'cargo run' >/tmp/terminal-screenshot-alacritty.log 2>&1 &
terminal_pid=$!
printf '%s\n' "$terminal_pid" >/tmp/terminal-screenshot-alacritty.pid
for ((i = 0; i < 100; i++)); do
  mapfile -t windows < <(DISPLAY=:99 xdotool search --all --onlyvisible \
    --pid "$terminal_pid" --class '^codex-terminal-screenshot$' 2>/dev/null)
  if (( ${#windows[@]} == 1 )); then
    win=${windows[0]}
    break
  fi
  kill -0 "$terminal_pid" 2>/dev/null || break
  sleep 0.05
done
if [[ -z "$win" ]]; then
  echo 'No unique test terminal appeared.' >&2
  cat /tmp/terminal-screenshot-alacritty.log >&2
  exit 1
fi
printf '%s\n' "$win" >/tmp/terminal-screenshot-window.id

# Wait up to 60 seconds for cargo to replace itself with the actual application.
for ((i = 0; i < 300; i++)); do
  app_pid=$(pgrep -P "$terminal_pid" -f '(^|/)mypowers-ratatui$' || true)
  [[ -n "$app_pid" ]] && break
  kill -0 "$terminal_pid" 2>/dev/null || break
  sleep 0.2
done
if [[ -z "$app_pid" ]]; then
  echo 'The application did not start. Inspect the held terminal and Alacritty log.' >&2
  cat /tmp/terminal-screenshot-alacritty.log >&2
  exit 1
fi
printf '%s\n' "$app_pid" >/tmp/terminal-screenshot-app.pid
sleep 0.5
DISPLAY=:99 import -window "$win" "$output"
test -s "$output"
printf 'Screenshot: %s\nXvfb PID: %s; terminal PID: %s; window: %s\n' "$output" "$xvfb_pid" "$terminal_pid" "$win"
echo 'Press Enter to exit the application and close the test display.'
read -r finished || true
