"""Persistent asyncio dashboard with independent streams, input and rendering."""

import argparse
import asyncio
import os
import signal
import sys
from typing import Any

from rich.console import Console
from rich.live import Live

from mypowers.client import Client
from mypowers.config import ClientConfig, client_config
from mypowers.contracts import AppError, Output, Status
from mypowers_tui.dashboard import Dashboard
from mypowers_tui.input import Decoder, InputEvent
from mypowers_tui.terminal import Terminal


class Application:
    def __init__(self, client: Client, dashboard: Dashboard, stop: asyncio.Event):
        self.client, self.dashboard, self.stop = client, dashboard, stop
        self.tasks: set[asyncio.Task[None]] = set()
        self.press: tuple[str, int] | None = None

    def spawn(self, coroutine: Any) -> None:
        task: asyncio.Task[None] = asyncio.create_task(coroutine)
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)

    async def state_stream(self) -> None:
        while not self.stop.is_set():
            try:
                async for message in self.client.stream():
                    if message.type in {"snapshot", "state"} and message.data:
                        self.dashboard.update(Status.model_validate(message.data))
                    elif message.type == "command" and message.data:
                        result = message.data
                        self.dashboard.notice = f"{result['output'].upper()}: {result['status']}"
            except Exception:
                self.dashboard.server_connected = False
                self.dashboard.notice = "Server connection lost. Reconnecting..."
            try:
                await asyncio.wait_for(self.stop.wait(), 2)
            except TimeoutError:
                pass

    async def log_stream(self) -> None:
        while not self.stop.is_set():
            try:
                async for message in self.client.stream(logs=True, min_level="DEBUG"):
                    if message.type == "log" and message.data:
                        self.dashboard.logs.append(message.data)
                    if message.type == "gap":
                        self.dashboard.notice = "Log history gap; showing available records."
            except Exception:
                pass
            try:
                await asyncio.wait_for(self.stop.wait(), 3)
            except TimeoutError:
                pass

    def event(self, event: InputEvent) -> None:
        dash = self.dashboard
        if event.kind == "press":
            if event.button & 3 == 0:
                hit = next((hit for hit in dash.hits if hit.contains(event.x, event.y)), None)
                self.press = (hit.action, dash.generation) if hit else None
            return
        if event.kind == "release":
            pressed, self.press = self.press, None
            if pressed and pressed[1] == dash.generation:
                hit = next((hit for hit in dash.hits if hit.contains(event.x, event.y)), None)
                if hit and hit.action == pressed[0]:
                    self.activate(hit.action)
            return
        if event.kind == "scroll":
            dash.scroll = max(0, min(999, dash.scroll + (-3 if event.button & 1 else 3)))
            return
        if event.kind != "key":
            return
        key = event.key
        if key in {"q", "quit"}:
            self.stop.set()
        elif key in {"tab", "backtab"}:
            dash.focus = (dash.focus + (1 if key == "tab" else -1)) % max(1, len(dash.hits))
        elif key in {"enter", "space"} and dash.hits:
            self.activate(dash.hits[dash.focus % len(dash.hits)].action)
        elif key in {"d", "l", "?"}:
            self.activate({"d": "dashboard", "l": "logs", "?": "help"}[key])
        elif key in {"up", "down", "pageup", "pagedown"}:
            dash.scroll = max(
                0, min(999, dash.scroll + {"up": 1, "down": -1, "pageup": 10, "pagedown": -10}[key])
            )
        elif key == "f":
            levels = ["DEBUG", "INFO", "WARNING", "ERROR"]
            dash.level = levels[(levels.index(dash.level) + 1) % len(levels)]

    def activate(self, action: str) -> None:
        dash = self.dashboard
        if action == "quit":
            self.stop.set()
        elif action in {"dashboard", "logs", "help"}:
            dash.view = action
            self.press = None
        elif action in {output.value for output in Output}:
            if not dash.allowed() or dash.status is None or dash.status.telemetry.sample is None:
                dash.notice = "Fresh station data required; controls unavailable."
                return
            desired = not getattr(dash.status.telemetry.sample, action + "_enabled")
            dash.pending = f"{action.upper()} {'On' if desired else 'Off'}"
            # Snapshot and target are captured once. Reconnection never queues/replays the intent.
            self.spawn(self.control(Output(action), desired, dash.status))
        elif action in {"retry", "pause", "debug"}:
            if dash.pending:
                return
            self.spawn(self.operation(action))

    async def control(self, output: Output, desired: bool, snapshot: Status) -> None:
        dash = self.dashboard
        try:
            admitted = await self.client.admit(output, desired, snapshot)
            result = await self.client.wait_command(admitted)
            dash.notice = f"{output.value.upper()} {result.status} | {result.command_id}"
            if result.status == "unconfirmed":
                dash.notice = f"Outcome uncertain | {result.command_id}; do not replay."
        except AppError as error:
            dash.notice = error.message
        except Exception:
            dash.notice = "Control response unavailable. Refresh status before further action."
        finally:
            dash.pending = None

    async def operation(self, action: str) -> None:
        dash = self.dashboard
        try:
            if action == "retry":
                await self.client.request("POST", "/connection/retry", body={})
            elif action == "pause":
                desired = (
                    "running"
                    if dash.status and dash.status.connection.desired == "paused"
                    else "paused"
                )
                await self.client.request("PUT", "/connection", body={"desired": desired})
            elif dash.status and dash.status.logging.get("effective_level") == "DEBUG":
                await self.client.request("DELETE", "/runtime/log-level")
            else:
                await self.client.request("PUT", "/runtime/log-level", body={"level": "DEBUG"})
            dash.notice = f"{action.capitalize()} request completed."
        except AppError as error:
            dash.notice = error.message
        except Exception:
            dash.notice = "Server request failed."

    async def close(self) -> None:
        for task in self.tasks:
            task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)


async def run(config: ClientConfig, no_mouse: bool = False) -> None:
    stop = asyncio.Event()
    console = Console(no_color=config.no_color or "NO_COLOR" in os.environ, markup=False)
    dashboard, decoder = Dashboard(), Decoder()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stop.set)
    try:
        with Terminal(mouse=not no_mouse):
            async with Client(config) as client:
                app = Application(client, dashboard, stop)
                app.spawn(app.state_stream())
                app.spawn(app.log_stream())

                def input_ready() -> None:
                    try:
                        raw = os.read(sys.stdin.fileno(), 4096)
                        if not raw:
                            stop.set()
                        for event in decoder.feed(raw):
                            app.event(event)
                    except OSError:
                        stop.set()

                loop.add_reader(sys.stdin.fileno(), input_ready)
                try:
                    with Live(
                        dashboard.render(console.width, console.height),
                        console=console,
                        screen=True,
                        auto_refresh=False,
                        transient=True,
                    ) as live:
                        while not stop.is_set():
                            live.update(
                                dashboard.render(console.width, console.height), refresh=True
                            )
                            try:
                                await asyncio.wait_for(stop.wait(), 0.25)
                            except TimeoutError:
                                pass
                finally:
                    loop.remove_reader(sys.stdin.fileno())
                    await app.close()
    finally:
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.remove_signal_handler(sig)


def launch(config: ClientConfig, no_mouse: bool = False) -> None:
    try:
        asyncio.run(run(config, no_mouse))
    except (ValueError, OSError) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(2) from None
    except KeyboardInterrupt:
        raise SystemExit(130) from None
    except Exception:
        print(
            "TUI stopped after an application error; terminal settings restored.", file=sys.stderr
        )
        raise SystemExit(1) from None


def main() -> None:
    parser = argparse.ArgumentParser(prog="mypowers-tui")
    parser.add_argument("--env-file")
    parser.add_argument("--server")
    parser.add_argument("--token-file")
    parser.add_argument("--ca-file")
    parser.add_argument("--timezone")
    parser.add_argument("--utc", action="store_true")
    parser.add_argument("--timeout", type=float)
    parser.add_argument("--no-color", action="store_true", default=None)
    parser.add_argument("--no-mouse", action="store_true")
    args = parser.parse_args()
    try:
        config = client_config(
            args.env_file,
            server=args.server,
            token_file=args.token_file,
            ca_file=args.ca_file,
            timezone="UTC" if args.utc else args.timezone,
            timeout=args.timeout,
            no_color=args.no_color,
        )
    except (ValueError, OSError):
        print(
            "Invalid client configuration. Check URL, token permissions and timezone.",
            file=sys.stderr,
        )
        raise SystemExit(2) from None
    launch(config, args.no_mouse)
