"""One-shot operations, clean JSON, explicit follow, and stable exit codes."""

import argparse
import asyncio
import json
import os
import re
import sys
from datetime import UTC, datetime, timedelta
from typing import Any
from zoneinfo import ZoneInfo

from rich.console import Console
from rich.table import Table
from rich.text import Text

from mypowers import __version__
from mypowers.client import Client
from mypowers.config import ClientConfig, client_config
from mypowers.contracts import AppError, Level, Output, safe_text


def common_options() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(add_help=False, argument_default=argparse.SUPPRESS)
    parser.add_argument("--env-file")
    parser.add_argument("--server")
    parser.add_argument("--token-file")
    parser.add_argument("--ca-file")
    parser.add_argument("--timeout", type=float)
    parser.add_argument("--no-color", action="store_true")
    parser.add_argument("--timezone")
    parser.add_argument("--utc", action="store_true")
    parser.add_argument("--json", action="store_true")
    return parser


def parser() -> argparse.ArgumentParser:
    shared = common_options()
    root = argparse.ArgumentParser(prog="mypowers", parents=[shared])
    root.add_argument("--version", action="version", version=__version__)
    commands = root.add_subparsers(dest="command", required=True)
    status = commands.add_parser("status", parents=[shared])
    status.add_argument("--require-live", action="store_true")
    commands.add_parser("capabilities", parents=[shared])
    for output in Output:
        action = commands.add_parser(output.value, parents=[shared])
        action.add_argument("desired", choices=["on", "off"])
    command = commands.add_parser("command", parents=[shared])
    command.add_argument("command_id")
    for name in ("history", "logs"):
        query = commands.add_parser(name, parents=[shared])
        query.add_argument("--since")
        query.add_argument("--until")
        query.add_argument("--limit", type=int, default=1000 if name == "history" else 100)
        query.add_argument("--cursor")
        if name == "logs":
            query.add_argument("--tail", type=int)
            query.add_argument("--level", choices=[level.value for level in Level], default="DEBUG")
            query.add_argument("--follow", action="store_true")
    debug = commands.add_parser("debug", parents=[shared])
    debug.add_argument("desired", choices=["on", "off"])
    debug.add_argument("--duration")
    connection = commands.add_parser("connection", parents=[shared])
    connection.add_argument("desired", choices=["pause", "resume", "retry"])
    tui = commands.add_parser("tui", parents=[shared])
    tui.add_argument("--no-mouse", action="store_true")
    return root


def settings(args: argparse.Namespace) -> ClientConfig:
    names = {
        name: getattr(args, name, None)
        for name in ("server", "token_file", "ca_file", "timeout", "no_color", "timezone")
    }
    if getattr(args, "utc", False):
        names["timezone"] = "UTC"
    return client_config(getattr(args, "env_file", None), **names)


def duration(value: str) -> float:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)([smhd]?)", value)
    if not match:
        raise ValueError("Use a duration such as 30s, 15m or 1h.")
    result = float(match[1]) * {"": 1, "s": 1, "m": 60, "h": 3600, "d": 86400}[match[2]]
    if not 0 < result <= 86400:
        raise ValueError("Duration must be greater than zero and at most one day.")
    return result


def query_time(value: str | None, server_time: str, config: ClientConfig) -> str | None:
    if value is None:
        return None
    if re.fullmatch(r"\d+(?:\.\d+)?[smhd]", value):
        result = datetime.fromisoformat(server_time.replace("Z", "+00:00")) - timedelta(
            seconds=duration(value)
        )
    else:
        result = datetime.fromisoformat(value.replace("Z", "+00:00"))
        if result.tzinfo is None:
            if not config.timezone:
                raise ValueError(
                    "Local timestamps require --timezone or use an aware UTC timestamp."
                )
            result = result.replace(tzinfo=ZoneInfo(config.timezone))
    return result.astimezone(UTC).isoformat().replace("+00:00", "Z")


def display_time(value: str, config: ClientConfig) -> str:
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    local = parsed.astimezone(ZoneInfo(config.timezone)) if config.timezone else parsed.astimezone()
    return local.strftime("%Y-%m-%d %H:%M:%S %Z")


def emit(data: dict[str, Any], args: argparse.Namespace, config: ClientConfig) -> None:
    if getattr(args, "json", False):
        print(json.dumps(data, ensure_ascii=True, separators=(",", ":")))
        return

    def clean(value: Any) -> Any:
        if isinstance(value, str):
            return (
                safe_text(value).replace(config.api_token, "[redacted]")
                if config.api_token
                else safe_text(value)
            )
        if isinstance(value, dict):
            return {k: clean(v) for k, v in value.items()}
        if isinstance(value, list):
            return [clean(v) for v in value]
        return value

    data = clean(data)
    console = Console(
        no_color=config.no_color or "NO_COLOR" in os.environ, markup=False, highlight=False
    )
    if "error" in data:
        console.print(Text(data["error"]["message"], style="red"))
    elif "telemetry" in data:
        sample = data["telemetry"]["sample"]
        console.print(
            Text(
                f"{data['device']['name']}  |  {data['connection']['phase']}"
                f"  |  {data['telemetry']['state']}"
            )
        )
        if sample:
            minutes = sample["remaining_minutes"]
            console.print(
                f"Battery {sample['battery_percent']}%"
                f"   Input {sample['input_power_w']} W   Output {sample['output_power_w']} W"
            )
            console.print(
                f"AC {'On' if sample['ac_enabled'] else 'Off'}"
                f"   DC {'On' if sample['dc_enabled'] else 'Off'}"
                f"   Lamps {'On' if sample['light_enabled'] else 'Off'}"
            )
            age = data["telemetry"]["age_seconds"]
            console.print(
                f"Station estimate {minutes // 60}h {minutes % 60:02d}m   Sample age {age:.1f}s"
            )
        else:
            console.print("No station sample available.")
    elif "command_id" in data:
        console.print(
            Text(f"{data['output'].upper()} {data['status']}  |  Command {data['command_id']}")
        )
        if data.get("reason_code"):
            console.print(Text(str(data["reason_code"])))
        if data["status"] == "unconfirmed":
            console.print("The output may have changed. Query the command/status; do not replay.")
    elif "items" in data:
        table = Table(box=None, padding=(0, 1))
        if args.command == "logs":
            for title in ("Time", "Level", "Event", "Message"):
                table.add_column(title)
            for record in data["items"]:
                table.add_row(
                    *(
                        Text(str(value))
                        for value in (
                            display_time(record["timestamp"], config),
                            record["level"],
                            record["event"],
                            record["message"],
                        )
                    )
                )
        else:
            for title in ("Received UTC", "Battery", "Input W", "Output W", "Segment"):
                table.add_column(title)
            for record in data["items"]:
                table.add_row(
                    *(
                        Text(str(value))
                        for value in (
                            display_time(
                                datetime.fromtimestamp(
                                    record["received_at_ms"] / 1000, UTC
                                ).isoformat(),
                                config,
                            ),
                            record["battery_percent"],
                            record["input_power_w"],
                            record["output_power_w"],
                            record["segment_id"],
                        )
                    )
                )
        console.print(table)
        if data.get("next_cursor"):
            console.print(Text("Next cursor: " + data["next_cursor"]))
    else:
        console.print_json(json.dumps(data))


def command_exit(data: dict[str, Any]) -> int:
    status = data.get("status")
    if status == "unconfirmed":
        return 6
    if status == "rejected":
        return 5
    if status == "failed":
        return 1
    return 0


async def run(args: argparse.Namespace, config: ClientConfig) -> int:
    async with Client(config) as client:
        name = args.command
        if name == "status":
            snapshot = await client.status()
            data = snapshot.model_dump(mode="json")
            emit(data, args, config)
            return 5 if args.require_live and snapshot.telemetry.state != "live" else 0
        if name in {output.value for output in Output}:
            command = await client.admit(Output(name), args.desired == "on")
            result = await client.wait_command(command)
            data = result.model_dump(mode="json")
        elif name == "command":
            data = (await client.command(args.command_id)).model_dump(mode="json")
        elif name == "capabilities":
            data = await client.request("GET", "/capabilities")
        elif name in {"history", "logs"}:
            server_time = (await client.status()).server_time
            params: dict[str, Any] = {
                "since": query_time(args.since, server_time, config),
                "until": query_time(args.until, server_time, config),
                "limit": args.limit,
                "cursor": args.cursor,
            }
            if name == "logs":
                params["min_level"] = args.level
                params["tail"] = args.tail
                if not args.since and not args.until and not args.cursor:
                    params["tail"] = args.tail or 10
            data = await client.request("GET", "/" + name, params=params)
            emit(data, args, config)
            if name == "logs" and args.follow:
                stream_params = {"min_level": args.level}
                if data.get("next_cursor"):
                    stream_params["cursor"] = data["next_cursor"]
                async for message in client.stream(logs=True, **stream_params):
                    if message.type == "log" and message.data:
                        if getattr(args, "json", False):
                            print(json.dumps(message.data, ensure_ascii=True), flush=True)
                        else:
                            emit({"items": [message.data]}, args, config)
            return 0
        elif name == "debug":
            if args.desired == "off":
                data = await client.request("DELETE", "/runtime/log-level")
            else:
                body: dict[str, Any] = {"level": "DEBUG"}
                if args.duration:
                    body["duration_seconds"] = duration(args.duration)
                data = await client.request("PUT", "/runtime/log-level", body=body)
        elif name == "connection":
            if args.desired == "retry":
                data = await client.request("POST", "/connection/retry", body={})
            else:
                data = await client.request(
                    "PUT",
                    "/connection",
                    body={"desired": "paused" if args.desired == "pause" else "running"},
                )
        else:
            raise ValueError("Unknown command.")
        emit(data, args, config)
        return command_exit(data)


def main() -> None:
    args = parser().parse_args()
    try:
        config = settings(args)
        if args.command == "tui":
            try:
                from mypowers_tui.main import launch
            except ModuleNotFoundError:
                print("Install mypowers[tui].", file=sys.stderr)
                raise SystemExit(2) from None
            launch(config, no_mouse=args.no_mouse)
            return
        raise SystemExit(asyncio.run(run(args, config)))
    except AppError as error:
        emit(error.payload(), args, config)
        code = (
            6
            if error.code == "admission_unknown"
            else (
                3
                if error.status == 0
                else 4
                if error.status in {401, 403}
                else 5
                if error.status in {409, 503}
                else 1
            )
        )
        raise SystemExit(code) from None

    except BrokenPipeError:
        os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        raise SystemExit(0) from None

    except (ValueError, OSError) as error:
        print(f"Client configuration/usage error: {type(error).__name__}", file=sys.stderr)
        raise SystemExit(2) from None
    except KeyboardInterrupt:
        raise SystemExit(130) from None
