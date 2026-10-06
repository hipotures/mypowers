"""Explicit API-based hardware runner; this file is never collected by pytest."""

import argparse
import asyncio
import json
import os
import platform
import signal
import socket
import subprocess
import sys
import time
from importlib.metadata import version
from pathlib import Path
from typing import Any

from mypowers.client import Client
from mypowers.config import ClientConfig, environment
from mypowers.config.server import load
from mypowers.contracts import Output, Status, utc_now
from mypowers.protocol import MASKS, PROFILES

GRAY = [12, 14, 15, 13, 29, 31, 30, 28]


async def fresh(client: Client, budget: float = 30) -> Status:
    deadline = time.monotonic() + budget
    while time.monotonic() < deadline:
        snapshot = await client.status()
        if snapshot.telemetry.state == "live" and snapshot.controls.allowed:
            return snapshot
        await asyncio.sleep(0.2)
    raise RuntimeError("No live qualified hardware status before deadline.")


async def change(client: Client, snapshot: Status, target: int, results: dict[str, Any]) -> Status:
    sample = snapshot.telemetry.sample
    assert sample is not None
    source = sample.status_flags
    mask = source ^ target
    if mask not in MASKS.values() or source not in PROFILES or target not in PROFILES:
        raise RuntimeError("Transition is not a qualified single-switch edge.")
    output = next(output for output, value in MASKS.items() if value == mask)
    command = await client.admit(output, bool(target & mask), snapshot)
    admitted = time.monotonic()
    result = await client.wait_command(command)
    results["commands"].append(
        {
            "admitted_at": utc_now(),
            "duration_seconds": time.monotonic() - admitted,
            "source": source,
            "target": target,
            "result": result.model_dump(mode="json"),
        }
    )
    if result.status != "confirmed" or len(result.confirmation_sequences) != 2:
        raise RuntimeError(f"Command {result.command_id}: {result.status}; do not replay.")
    current = await fresh(client)
    if current.telemetry.sample is None or current.telemetry.sample.status_flags != target:
        raise RuntimeError("Unexpected complete state after confirmed transition.")
    return current


async def restore(client: Client, original: int, expected: int, results: dict[str, Any]) -> None:
    snapshot = await fresh(client, 3)
    sample = snapshot.telemetry.sample
    if sample is None or sample.status_flags != expected:
        raise RuntimeError(
            "Manual restore required: state differs from the known test progression."
        )
    for output in Output:
        mask = MASKS[output]
        current = snapshot.telemetry.sample
        assert current is not None
        if bool(current.status_flags & mask) != bool(original & mask):
            target = (
                (current.status_flags | mask) if original & mask else current.status_flags & ~mask
            )
            snapshot = await change(client, snapshot, target, results)
    results["restoration"] = "PASS: exact starting flags observed"


async def monitor(client: Client, pid: int, seconds: float, results: dict[str, Any]) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        before = time.monotonic()
        snapshot = await client.status()
        latency = time.monotonic() - before
        process_status = await asyncio.to_thread(Path(f"/proc/{pid}/status").read_text)
        process_stat = await asyncio.to_thread(Path(f"/proc/{pid}/stat").read_text)
        rss = next(line for line in process_status.splitlines() if line.startswith("VmRSS:"))
        stats = process_stat.split()
        results["resource_samples"].append(
            {
                "timestamp": utc_now(),
                "status_latency_seconds": latency,
                "rss_kib": int(rss.split()[1]),
                "cpu_ticks": int(stats[13]) + int(stats[14]),
                "phase": snapshot.connection.phase,
                "telemetry": snapshot.telemetry.state,
                "counters": snapshot.diagnostics,
            }
        )
        results["final"] = snapshot.model_dump(mode="json")
        await asyncio.sleep(min(10, max(0, deadline - time.monotonic())))


async def acceptance(args: argparse.Namespace) -> dict[str, Any]:
    output = await asyncio.to_thread(Path(args.output).resolve)
    await asyncio.to_thread(output.mkdir, parents=True, exist_ok=False)
    values = environment(args.env_file)
    writes = args.allow_output_changes or values.get("MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES") == "1"
    if writes and (not args.loads_safely_interruptible or not args.ordinary_common_lamp_mode):
        raise ValueError(
            "Output opt-in also requires safe-load and ordinary common-lamp operator declarations."
        )
    cfg = load(args.env_file, args.config)
    if cfg.backend != "ble":
        raise ValueError("Hardware acceptance requires the explicitly configured BLE backend.")
    if cfg.environment != "development":
        raise ValueError(
            "Use a dedicated development validation configuration with isolated paths."
        )
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    cfg.data_dir, cfg.log_dir, cfg.runtime_dir = output / "data", output / "logs", output / "run"
    cfg.api.port = port
    selected = output / "server.json"
    # The loader uses safe YAML, for which JSON is a strict subset.
    selected.write_text(json.dumps(cfg.model_dump(mode="json"), indent=2) + "\n")
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYPOWERS_")}
    if cfg.api_token:
        secret = output / "api-token"
        secret.touch(mode=0o600)
        secret.write_text(cfg.api_token + "\n")
        env["MYPOWERS_API_TOKEN_FILE"] = str(secret)
    results: dict[str, Any] = {
        "started_at": utc_now(),
        "versions": {
            name: version(name)
            for name in (
                "mypowers",
                "bleak",
                "dbus-fast",
                "fastapi",
                "uvicorn",
                "aiosqlite",
                "rich",
                "websockets",
            )
        },
        "python": platform.python_version(),
        "commands": [],
        "resource_samples": [],
        "write_acceptance": "NOT RUN: no opt-in" if not writes else "pending",
        "read_acceptance": "pending",
        "physical_observations": "NOT RUN: operator/camera observation is separate from BLE",
        "configuration": cfg.model_dump(mode="json"),
    }
    client_config = ClientConfig(server=f"http://127.0.0.1:{port}", api_token=cfg.api_token)
    bindir = Path(sys.executable).parent
    daemon: subprocess.Popen[bytes] | None = None
    stderr = (output / "daemon.stderr").open("wb")
    expected: int | None = None
    original: int | None = None
    stop_requested = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop_requested.set)
    try:
        daemon = await asyncio.to_thread(
            subprocess.Popen,
            [str(bindir / "mypowersd"), "--config", str(selected)],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=stderr,
        )
        async with Client(client_config) as client:
            deadline = time.monotonic() + 30
            while True:
                try:
                    snapshot = await fresh(client, 1)
                    break
                except Exception:
                    if time.monotonic() > deadline or daemon.poll() is not None:
                        raise RuntimeError(
                            "Station unavailable; inspect daemon diagnostics."
                        ) from None
                    await asyncio.sleep(0.2)
            results["initial"] = snapshot.model_dump(mode="json")
            assert snapshot.telemetry.sample is not None
            original = expected = snapshot.telemetry.sample.status_flags
            client_env = {**env, "MYPOWERS_SERVER_URL": client_config.server}
            # Actual installed CLI executions, independent of server implementation internals.
            for args_list in (
                ["status"],
                ["status", "--json"],
                ["history", "--json"],
                ["logs", "--tail", "10", "--json"],
                ["debug", "on", "--json"],
                ["debug", "off", "--json"],
            ):
                process = await asyncio.to_thread(
                    subprocess.run,
                    ["mypowers", *args_list],
                    env=client_env,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                results.setdefault("cli", []).append(
                    {
                        "args": args_list,
                        "exit_code": process.returncode,
                        "stdout": process.stdout,
                        "stderr": process.stderr,
                    }
                )
                if process.returncode:
                    raise RuntimeError("Installed CLI read/diagnostic acceptance failed.")
            stream = client.stream()
            results["websocket_snapshot"] = (await anext(stream)).model_dump(mode="json")
            await stream.aclose()
            results["read_acceptance"] = "PASS"
            if writes:
                rotation = GRAY.index(original)
                cycle = GRAY[rotation:] + GRAY[:rotation] + [original]
                try:
                    for target in cycle[1:]:
                        if stop_requested.is_set():
                            raise RuntimeError("Interrupted hardware test.")
                        snapshot = await change(client, snapshot, target, results)
                        expected = target
                    results["write_acceptance"] = "PASS"
                finally:
                    if original is not None and expected is not None and original != expected:
                        await restore(client, original, expected, results)
            # Explicit session release/reacquisition through API, without any output frame.
            await client.request("PUT", "/connection", body={"desired": "paused"})
            await asyncio.sleep(0.5)
            await client.request("PUT", "/connection", body={"desired": "running"})
            results["reacquisition"] = (await fresh(client)).model_dump(mode="json")
            monitoring = asyncio.create_task(monitor(client, daemon.pid, args.seconds, results))
            stopped = asyncio.create_task(stop_requested.wait())
            done, _ = await asyncio.wait({monitoring, stopped}, return_when=asyncio.FIRST_COMPLETED)
            if stopped in done:
                monitoring.cancel()
                results["soak"] = "NOT RUN: interrupted"
            else:
                await monitoring
                results["soak"] = (
                    "PASS"
                    if args.seconds >= 3600
                    else "NOT RUN: requested duration below 60 minutes"
                )
            stopped.cancel()
            await asyncio.gather(monitoring, stopped, return_exceptions=True)
            final = await client.status()
            results["final"] = final.model_dump(mode="json")
            if final.telemetry.sample and final.telemetry.sample.status_flags != original:
                results["manual_action"] = (
                    "Output state differs from baseline; inspect it manually."
                )
    except Exception as error:
        results["error"] = str(error)
        if results["read_acceptance"] == "pending":
            results["read_acceptance"] = "FAIL"
        if results["write_acceptance"] == "pending":
            results["write_acceptance"] = "FAIL"
    finally:
        if daemon:
            daemon.send_signal(signal.SIGTERM)
            try:
                await asyncio.to_thread(daemon.wait, timeout=12)
            except subprocess.TimeoutExpired:
                daemon.kill()
                await asyncio.to_thread(daemon.wait)
                results["shutdown"] = "FAIL: required forced termination"
            results["daemon_exit_code"] = daemon.returncode
        stderr.close()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.remove_signal_handler(sig)
        results["finished_at"] = utc_now()
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        lines = [
            "# Hardware acceptance",
            f"Started: {results['started_at']}",
            f"Read: {results['read_acceptance']}",
            f"Control: {results['write_acceptance']}",
            f"Soak: {results.get('soak', 'NOT RUN')}",
            f"Final: {json.dumps(results.get('final', {}).get('telemetry'))}",
        ]
        (output / "REPORT.md").write_text("\n\n".join(lines) + "\n")
    return results


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file")
    parser.add_argument("--config")
    parser.add_argument("--output", required=True)
    parser.add_argument("--seconds", type=float, default=3600)
    parser.add_argument("--allow-output-changes", action="store_true")
    parser.add_argument("--loads-safely-interruptible", action="store_true")
    parser.add_argument("--ordinary-common-lamp-mode", action="store_true")
    args = parser.parse_args()
    if not 0 < args.seconds <= 86400:
        parser.error("seconds must be greater than zero and at most one day")
    result = asyncio.run(acceptance(args))
    print(
        json.dumps({key: result[key] for key in ("read_acceptance", "write_acceptance")}, indent=2)
    )
    raise SystemExit(1 if "error" in result else 0)


if __name__ == "__main__":
    main()
