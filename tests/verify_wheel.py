"""Verify server-only wheels and native clients from outside the checkout."""

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
        assert any(name.startswith("mypowers/") for name in names)
        assert not any(name.startswith(("mypowers_cli/", "mypowers_tui/")) for name in names)
        assert not any(
            name.startswith(("tests/", "docs/", "config/", ".env", ".local/")) for name in names
        )
    results = {}
    with tempfile.TemporaryDirectory(prefix="mypowers-wheel-") as temporary:
        directory = Path(temporary)
        for kind in ("server", "base"):
            target = directory / kind
            execute(["uv", "venv", "--python", sys.executable, str(target)], directory)
            python = target / "bin/python"
            install = ["uv", "pip", "install", "--python", str(python)]
            if args.offline:
                install.append("--offline")
            if kind == "server":
                execute(
                    [
                        *install,
                        "--require-hashes",
                        "-r",
                        str(root / "deploy/requirements-server.txt"),
                    ],
                    directory,
                )
                execute([*install, "--no-deps", str(wheel)], directory)
            else:
                execute([*install, str(wheel)], directory)
            absent = ["rich", "httpx"]
            if kind == "base":
                absent += ["bleak", "dbus_fast", "fastapi", "uvicorn", "aiosqlite", "yaml"]
            code = (
                "import importlib.util; assert all("
                "importlib.util.find_spec(name) is None for name in " + repr(absent) + ")"
            )
            execute([str(python), "-c", code], directory)
            assert not (target / "bin/mypowers").exists()
            if kind == "server":
                execute([str(target / "bin/mypowersd"), "--help"], directory)
            else:
                hint = subprocess.run(
                    [str(target / "bin/mypowersd")], cwd=directory, capture_output=True, text=True
                )
                assert hint.returncode == 2 and "Install mypowers[server]" in hint.stderr
                assert "Traceback" not in hint.stderr
            results[kind] = {"result": "PASS", "absent_dependencies": absent}
        for name in ("mypowers", "mypowers-tui"):
            binary = root / "frontends/tui/target/debug" / name
            assert binary.is_file(), "Build the native clients before wheel verification."
            execute([str(binary), "--help"], directory)
        results["native_clients"] = {"result": "PASS", "outside_checkout": True}
    if args.result_file:
        Path(args.result_file).write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
