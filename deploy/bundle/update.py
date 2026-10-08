#!/usr/bin/env python3
"""Update an existing Supervisor installation, with health checks and code rollback."""

import fcntl
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

BUNDLE = Path(__file__).resolve().parent
CURRENT = Path("/opt/mypowers/current")
SUPERVISOR = "/etc/mypowers/supervisord.conf"
ENV_FILE = "/etc/mypowers/server.env"
# Run in the installed interpreter, loading credentials privately on the LXC.
HEALTH = """
import json
from urllib.request import Request, build_opener, ProxyHandler
from urllib.parse import urlsplit
from mypowers.config.server import load
c = load(env_file="/etc/mypowers/server.env")
h = {"Authorization": "Bearer " + c.api_token} if c.api_token else {}
h["Host"] = urlsplit(c.public_url).netloc if c.public_url else "localhost"
u = "http://127.0.0.1:" + str(c.api.port)
o = build_opener(ProxyHandler({}))
with o.open(Request(u + "/api/v1/status", headers=h), timeout=3) as r:
 s = json.load(r)
assert s["diagnostics"]["healthy"]
print(s["server_instance_id"])
"""


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def supervisor(action: str) -> None:
    run("supervisorctl", "-c", SUPERVISOR, action, "mypowers")


def switch(release: Path) -> None:
    pending = CURRENT.with_name("current.update")
    pending.unlink(missing_ok=True)
    pending.symlink_to(release)
    pending.replace(CURRENT)


def health(python: Path) -> str:
    result = subprocess.run(
        [str(python), "-c", HEALTH],
        cwd="/var/lib/mypowers",
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def activate(release: Path, previous: Path, old_instance: str) -> None:
    switch(release)
    try:
        supervisor("restart")
        deadline = time.monotonic() + 30
        while True:
            try:
                instance = health(release / "venv/bin/python")
                if instance and instance != old_instance:
                    break
            except (subprocess.CalledProcessError, OSError):
                pass
            if time.monotonic() >= deadline:
                raise RuntimeError("New daemon did not pass the authenticated health check.")
            time.sleep(1)
    except BaseException:
        switch(previous)
        supervisor("restart")
        # Report a failed rollback as a failure too, never imply recovery.
        health(previous / "venv/bin/python")
        raise


def main() -> None:
    if os.geteuid() != 0:
        raise SystemExit("Run as root.")
    if not CURRENT.is_symlink():
        raise SystemExit("No existing installation; use install.py for the first installation.")
    with Path("/run/mypowers-update.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        manifest = json.loads((BUNDLE / "manifest.json").read_text())
        if manifest["python_version"] != f"{sys.version_info.major}{sys.version_info.minor}":
            raise SystemExit("Python version differs from the bundle target.")
        if not re.fullmatch(r"[0-9a-f]{40}", manifest["commit"]):
            raise SystemExit("Invalid bundle commit.")
        for name, expected in manifest["files"].items():
            path = BUNDLE / name
            if (
                not path.resolve().is_relative_to(BUNDLE)
                or hashlib.sha256(path.read_bytes()).hexdigest() != expected
            ):
                raise SystemExit(f"Bundle checksum mismatch: {name}")
        previous = CURRENT.resolve(strict=True)
        old_instance = health(previous / "venv/bin/python")
        release = CURRENT.parent / "releases" / f"{manifest['commit'][:12]}-{time.time_ns()}"
        release.mkdir(parents=True)
        run(sys.executable, "-m", "venv", str(release / "venv"))
        python = release / "venv/bin/python"
        run(
            str(python),
            "-m",
            "pip",
            "install",
            "--no-index",
            "--find-links",
            str(BUNDLE / "wheels"),
            "--require-hashes",
            "-r",
            str(BUNDLE / "requirements.txt"),
        )
        wheels = list((BUNDLE / "wheels").glob("mypowers-*.whl"))
        if len(wheels) != 1:
            raise SystemExit("Bundle must contain exactly one mypowers wheel.")
        run(str(python), "-m", "pip", "install", "--no-index", "--no-deps", str(wheels[0]))
        run(str(release / "venv/bin/mypowersd"), "check-config", "--env-file", ENV_FILE)
        # SQLite backup is consistent even while the old daemon writes in WAL mode.
        backup_code = """
import sqlite3, sys
from mypowers.config.server import load
c = load(env_file="/etc/mypowers/server.env")
source = sqlite3.connect(str(c.data_dir / "mypowers.db"))
target = sqlite3.connect(sys.argv[1])
try:
 source.backup(target)
finally:
 target.close()
 source.close()
"""
        subprocess.run(
            [
                str(previous / "venv/bin/python"),
                "-c",
                backup_code,
                str(release / "database-before-update.db"),
            ],
            cwd="/var/lib/mypowers",
            check=True,
        )
        (release / "database-before-update.db").chmod(0o600)
        shutil.copyfile(BUNDLE / "manifest.json", release / "manifest.json")
        activate(release, previous, old_instance)
        print(f"Updated to {release}; previous release: {previous}")
        print("Daemon health verified. Station reconnection proceeds asynchronously.")


if __name__ == "__main__":
    main()
