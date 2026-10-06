#!/usr/bin/env python3
"""Install a verified offline daemon bundle into an Alpine Python LXC."""

import argparse
import hashlib
import json
import os
import pwd
import shutil
import subprocess
import sys
import time
from pathlib import Path

BUNDLE = Path(__file__).resolve().parent
BOOT_MARKER = "# MyPowers managed supervisor startup\n"


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def boot_script(original: str) -> str:
    if BOOT_MARKER in original:
        return original
    tail = "while True:\n    time.sleep(3600)"
    if (
        not original.rstrip().endswith(tail)
        or 'subprocess.run(["/usr/sbin/sshd"], check=True)' not in original
    ):
        raise ValueError("Unsupported startup script; refusing to replace it.")
    return (
        original[: original.index(tail)]
        + BOOT_MARKER
        + (
            "import pwd\n"
            'os.makedirs("/run/mypowers", exist_ok=True)\n'
            'account = pwd.getpwnam("mypowers")\n'
            'os.chown("/run/mypowers", account.pw_uid, account.pw_gid)\n'
            'os.execv("/usr/bin/supervisord", ["supervisord", "-n", "-c", '
            '"/etc/mypowers/supervisord.conf"])\n'
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hostname", required=True, help="HTTPS service hostname")
    parser.add_argument("--startup-python", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run as root.")
    if not args.hostname or any(
        c not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for c in args.hostname
    ):
        parser.error("Invalid hostname.")
    manifest = json.loads((BUNDLE / "manifest.json").read_text())
    if manifest["python_version"] != f"{sys.version_info.major}{sys.version_info.minor}":
        parser.error("Python version differs from the bundle target.")
    for name, expected in manifest["files"].items():
        path = BUNDLE / name
        if (
            not path.resolve().is_relative_to(BUNDLE)
            or hashlib.sha256(path.read_bytes()).hexdigest() != expected
        ):
            parser.error(f"Bundle checksum mismatch: {name}")
    original_boot = args.startup_python.read_text()
    replacement_boot = boot_script(original_boot)
    # OS packages are separate from the offline Python payload.
    run("apk", "add", "supervisor", "ca-certificates", "tzdata")
    try:
        account = pwd.getpwnam("mypowers")
    except KeyError:
        run("adduser", "-S", "-D", "-H", "-s", "/sbin/nologin", "mypowers")
        account = pwd.getpwnam("mypowers")
    for name in ("/var/lib/mypowers", "/var/log/mypowers", "/run/mypowers"):
        path = Path(name)
        path.mkdir(parents=True, exist_ok=True)
        os.chown(path, account.pw_uid, account.pw_gid)
    config = Path("/etc/mypowers")
    config.mkdir(exist_ok=True)
    release = Path("/opt/mypowers/releases") / f"{manifest['commit'][:12]}-{time.time_ns()}"
    release.mkdir(parents=True)
    run(sys.executable, "-m", "venv", str(release / "venv"))
    pip = str(release / "venv/bin/python")
    run(
        pip,
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
    wheel = next((BUNDLE / "wheels").glob("mypowers-*.whl"))
    run(pip, "-m", "pip", "install", "--no-index", "--no-deps", str(wheel))
    for name in ("config.yaml", "server.env"):
        target = config / name
        if not target.exists():
            contents = (BUNDLE / name).read_text()
            if name == "server.env":
                contents = contents.replace("https://mypower.efez.net", f"https://{args.hostname}")
            target.write_text(contents)
            target.chmod(0o640)
            os.chown(target, 0, account.pw_gid)
    token = config / "api-token"
    daemon = str(release / "venv/bin/mypowersd")
    if not token.exists():
        run(daemon, "token", "generate", "--output", str(token))
    token.chmod(0o600)
    os.chown(token, account.pw_uid, account.pw_gid)
    run(daemon, "check-config", "--env-file", str(config / "server.env"))
    current = Path("/opt/mypowers/current")
    previous = current.resolve() if current.is_symlink() else None
    pending = current.with_name("current.new")
    pending.symlink_to(release)
    pending.replace(current)
    shutil.copyfile(BUNDLE / "manifest.json", release / "manifest.json")
    shutil.copyfile(BUNDLE / "supervisord.conf", config / "supervisord.conf")
    caddy = Path("/etc/caddy/Caddyfile")
    if not caddy.with_suffix(".pre-mypowers").exists():
        shutil.copyfile(caddy, caddy.with_suffix(".pre-mypowers"))
    # Credentials stay in the service environment, never in this file.
    caddy.write_text(f"""{args.hostname} {{
    tls {{
        dns ovh {{
            endpoint {{$OVH_ENDPOINT}}
            application_key {{$OVH_APPLICATION_KEY}}
            application_secret {{$OVH_APPLICATION_SECRET}}
            consumer_key {{$OVH_CONSUMER_KEY}}
        }}
    }}
    reverse_proxy 127.0.0.1:8765
}}
""")
    run(
        "/bin/sh",
        "-c",
        ". /etc/conf.d/caddy; /usr/sbin/caddy validate "
        "--config /etc/caddy/Caddyfile --adapter caddyfile",
    )
    backup = args.startup_python.with_suffix(".py.pre-mypowers")
    if not backup.exists():
        shutil.copyfile(args.startup_python, backup)
    args.startup_python.write_text(replacement_boot)
    wrapper = Path("/usr/local/bin/mypowersd")
    if not wrapper.exists():
        wrapper.symlink_to("/opt/mypowers/current/venv/bin/mypowersd")
    print(f"Installed {release}; previous release: {previous}")
    print("Boot configured. Start: /usr/bin/supervisord -c /etc/mypowers/supervisord.conf")
    print("Status: supervisorctl -c /etc/mypowers/supervisord.conf status")


if __name__ == "__main__":
    main()
