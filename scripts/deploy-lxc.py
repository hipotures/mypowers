#!/usr/bin/env python3
"""Build and deploy a committed daemon release to the existing Alpine LXC over SSH."""

import argparse
import shlex
import subprocess
import sys
from pathlib import Path
from uuid import uuid4

ROOT = Path(__file__).resolve().parents[1]


def run(*args: str) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default="root@192.168.100.222", help="SSH user@host")
    parser.add_argument("--build-only", action="store_true", help="Build the archive without SSH")
    args = parser.parse_args()
    if args.target.startswith("-") or any(c.isspace() for c in args.target):
        parser.error("Invalid SSH target.")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT).strip():
        parser.error("Commit the changes first; deployment must identify the exact source commit.")
    run(sys.executable, str(ROOT / "scripts/build-install-bundle.py"))
    bundle = ROOT / "dist/mypowers-install-py314-musllinux_1_2_x86_64.tar.gz"
    if args.build_only:
        print(bundle)
        return
    remote = "/tmp/mypowers-deploy-" + uuid4().hex
    run("ssh", args.target, "mkdir -m 700 " + shlex.quote(remote))
    run("scp", str(bundle), args.target + ":" + remote + "/bundle.tar.gz")
    command = (
        "set -eu; cd "
        + shlex.quote(remote)
        + "; tar -xzf bundle.tar.gz; python3 mypowers-install/update.py"
    )
    # A failed/uncertain SSH result keeps the uploaded bundle for inspection.
    run("ssh", args.target, command)
    run("ssh", args.target, "rm -rf -- " + shlex.quote(remote))


if __name__ == "__main__":
    main()
