"""Keep entrypoints usable when an optional dependency set is absent."""

import importlib
import sys


def launch(module: str, extra: str) -> None:
    try:
        target = importlib.import_module(module)
    except ModuleNotFoundError as error:
        print(f"Install mypowers[{extra}] (missing dependency: {error.name}).", file=sys.stderr)
        raise SystemExit(2) from None
    target.main()


def daemon() -> None:
    launch("mypowers.daemon.main", "server")


def cli() -> None:
    launch("mypowers_cli.main", "cli")
