"""Build artifact installation smoke tests, run from outside the checkout."""

import argparse
import json
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


def execute(args: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--result-file")
    args = parser.parse_args()
    wheel = Path(args.wheel).resolve()
    root = Path(__file__).resolve().parents[1]
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()
        assert any(name.startswith("mypowers_cli/") for name in names)
        assert not any(name.startswith("mypowers_tui/") for name in names)
        assert not any(
            name.startswith(("tests/", "docs/", "config/", ".env", ".local/")) for name in names
        )
    results = {}
    with tempfile.TemporaryDirectory(prefix="mypowers-wheel-") as temporary:
        directory = Path(temporary)
        for kind in ("server", "client"):
            target = directory / kind
            execute(["uv", "venv", "--python", sys.executable, str(target)], directory)
            python = target / "bin/python"
            install = ["uv", "pip", "install", "--python", str(python)]
            if args.offline:
                install.append("--offline")
            execute(
                [*install, "--require-hashes", "-r", str(root / f"deploy/requirements-{kind}.txt")],
                directory,
            )
            execute([*install, "--no-deps", str(wheel)], directory)
            absent = (
                ["rich", "httpx"]
                if kind == "server"
                else ["bleak", "dbus_fast", "fastapi", "uvicorn", "aiosqlite", "yaml"]
            )
            code = (
                "import importlib.util; assert all("
                "importlib.util.find_spec(name) is None for name in " + repr(absent) + ")"
            )
            execute([str(python), "-c", code], directory)
            programs = ["mypowersd"] if kind == "server" else ["mypowers"]
            for name in programs:
                execute([str(target / "bin" / name), "--help"], directory)
            missing = "mypowers" if kind == "server" else "mypowersd"
            hint = subprocess.run(
                [str(target / "bin" / missing)], cwd=directory, capture_output=True, text=True
            )
            assert (
                hint.returncode == 2
                and "Install mypowers[" in hint.stderr
                and "Traceback" not in hint.stderr
            )
            results[kind] = {
                "result": "PASS",
                "absent_dependencies": absent,
                "outside_checkout": True,
                "missing_extra_hint": "PASS",
            }
    if args.result_file:
        Path(args.result_file).write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
