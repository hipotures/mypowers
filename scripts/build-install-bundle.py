#!/usr/bin/env python3
"""Build an offline Alpine/musl daemon installer from the project lockfile."""

import argparse
import hashlib
import json
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(*args: str) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-version", default="314")
    parser.add_argument("--platform", default="musllinux_1_2_x86_64")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="mypowers-bundle-") as directory:
        stage = Path(directory) / "mypowers-install"
        stage.mkdir()
        wheels = stage / "wheels"
        wheels.mkdir()
        run("uv", "build", "--wheel", "--out-dir", str(wheels))
        requirements = stage / "requirements.txt"
        run(
            "uv",
            "export",
            "--locked",
            "--extra",
            "server",
            "--no-dev",
            "--no-emit-project",
            "--output-file",
            str(requirements),
        )
        run(
            "uv",
            "run",
            "--with",
            "pip",
            "python",
            "-m",
            "pip",
            "download",
            "--require-hashes",
            "--only-binary=:all:",
            "--platform",
            args.platform,
            "--platform",
            args.platform.replace("musllinux_1_2_", "musllinux_1_1_"),
            "--python-version",
            args.python_version,
            "--implementation",
            "cp",
            "--abi",
            f"cp{args.python_version}",
            "-r",
            str(requirements),
            "-d",
            str(wheels),
        )
        for source, target in [
            ("deploy/bundle/install.py", "install.py"),
            ("deploy/bundle/supervisord.conf", "supervisord.conf"),
            ("config/production.example.yaml", "config.yaml"),
            (".env.production.example", "server.env"),
        ]:
            shutil.copyfile(ROOT / source, stage / target)
        manifest = {
            "python_version": args.python_version,
            "platform": args.platform,
            "commit": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip(),
            "files": {
                str(p.relative_to(stage)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in sorted(stage.rglob("*"))
                if p.is_file()
            },
        }
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        output = ROOT / "dist" / f"mypowers-install-py{args.python_version}-{args.platform}.tar.gz"
        output.parent.mkdir(exist_ok=True)
        with tarfile.open(output, "w:gz") as archive:
            archive.add(stage, arcname=stage.name)
        print(output)


if __name__ == "__main__":
    main()
