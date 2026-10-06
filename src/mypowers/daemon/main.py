"""Local management commands and single-worker foreground runner."""

import argparse
import copy
import json
import os
import secrets
import sys
from pathlib import Path

import uvicorn

from mypowers import __version__
from mypowers.api import create_app
from mypowers.config.server import load
from mypowers.diagnostics.redaction import RedactionFilter


def main() -> None:
    parser = argparse.ArgumentParser(prog="mypowersd")
    parser.add_argument("--version", action="version", version=__version__)
    # The same options before or after a management command have identical semantics.
    options = argparse.ArgumentParser(add_help=False, argument_default=argparse.SUPPRESS)
    options.add_argument("--env-file")
    options.add_argument("--config")
    options.add_argument("--bind-host")
    options.add_argument("--port", type=int)
    options.add_argument("--backend", choices=["ble", "simulated"])
    options.add_argument("--workers", type=int)
    parser = argparse.ArgumentParser(prog="mypowersd", parents=[options])
    parser.add_argument("--version", action="version", version=__version__)
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("check-config", parents=[options])
    generation = commands.add_parser("token")
    sub = generation.add_subparsers(dest="token_command", required=True)
    generate = sub.add_parser("generate")
    generate.add_argument("--output", required=True)
    args = parser.parse_args()
    for name in ("env_file", "config", "bind_host", "port", "backend"):
        if not hasattr(args, name):
            setattr(args, name, None)
    args.workers = getattr(args, "workers", 1)
    try:
        if args.command == "token":
            path = Path(args.output)
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(fd, "w") as file:
                file.write(secrets.token_urlsafe(32) + "\n")
            print(f"Token written to {path.resolve()}")
            return
        if args.workers != 1 or os.environ.get("WEB_CONCURRENCY", "1") != "1":
            raise ValueError("Exactly one worker is supported.")
        config = load(
            args.env_file,
            args.config,
            bind_host=args.bind_host,
            port=args.port,
            backend=args.backend,
        )
        if args.command == "check-config":
            print(json.dumps(config.model_dump(mode="json"), indent=2))
            return
        logging_config = copy.deepcopy(uvicorn.config.LOGGING_CONFIG)
        logging_config["filters"] = {
            "redaction": {
                "()": RedactionFilter,
                "secrets": tuple(
                    value
                    for value in (
                        config.api_token,
                        config.telegram_bot_token,
                        config.telegram_chat_id,
                    )
                    if value
                ),
            }
        }
        for handler in logging_config["handlers"].values():
            handler["filters"] = ["redaction"]
        uvicorn.run(
            create_app(config),
            host=config.api.bind_host,
            port=config.api.port,
            workers=1,
            access_log=False,
            log_config=logging_config,
            proxy_headers=True,
            forwarded_allow_ips="127.0.0.1,::1",
            ws_max_size=16384,
            timeout_graceful_shutdown=10,
        )
    except (ValueError, OSError, RuntimeError) as error:
        print(f"Configuration/startup error: {type(error).__name__}: {error}", file=sys.stderr)
        raise SystemExit(2) from None
