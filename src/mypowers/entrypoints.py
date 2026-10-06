"""Keep entrypoints usable when an optional dependency set is absent."""

import importlib
import sys


def launch(module: str, extra: str) -> None:
    try:
        target = importlib.import_module(module)
    except ModuleNotFoundError as error:
        print(f"Install mypowers[{extra}] (missing dependency: {error.name}).", file=sys.stderr)
        print(
            f"From the project directory, run: uv run --locked --extra {extra} mypowersd\n"
            f"For a standalone installation, run: pip install 'mypowers[{extra}]'",
            file=sys.stderr,
        )
        raise SystemExit(2) from None
    target.main()


def daemon() -> None:
    launch("mypowers.daemon.main", "server")
