# /// script
# requires-python = ">=3.12"
# dependencies = ["bleak==3.0.2", "dbus-fast==3.1.2"]
# ///
"""One supervised S300 three-switch matrix experiment, not an application API.

The fixed candidate set is experimental until its captures establish acceptance.
No arbitrary frame input, automatic write retry, or unsupported control exists.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

from bleak import BleakClient, BleakScanner

import control_allpowers as protocol

# Tuple order: AC, DC, common lamps. Rotate this cycle to the observed baseline.
GRAY_RX = (0x0C, 0x0E, 0x0F, 0x0D, 0x1D, 0x1F, 0x1E, 0x1C)
CANDIDATES = {0x0C: 0x18, 0x0E: 0x1A, 0x0D: 0x19, 0x0F: 0x1B,
              0x1C: 0x38, 0x1E: 0x3A, 0x1D: 0x39, 0x1F: 0x3B}
# These starting/restoration profiles were qualified before this experiment.
PRIOR_RX = frozenset((0x0C, 0x0E, 0x0F, 0x1E))


def state(flags: int) -> dict:
    return {"ac": bool(flags & 2), "dc": bool(flags & 1), "lamps": bool(flags & 16)}


def candidate_frame(rx: int) -> bytes:
    """Translate direction-specific masks and independently XOR all eight bytes."""
    if rx not in CANDIDATES:
        raise ValueError("outside the fixed three-switch experimental profile")
    tx = ((rx & 1) | (rx & 2) | ((rx & 4) << 1)
          | ((rx & 8) << 1) | ((rx & 16) << 1))
    if tx != CANDIDATES[rx]:
        raise ValueError("candidate mapping disagrees with preserved RX profile")
    body = bytes.fromhex("a56500b1010100") + bytes((tx,))
    checksum = 0
    for value in body:
        checksum ^= value
    return body + bytes((checksum,))


class Capture:
    def __init__(self, directory: Path):
        self.directory = directory
        directory.mkdir(parents=True, exist_ok=False)
        self.file = (directory / "session.jsonl").open("w")
        self.seq = 0
        self.latest = None
        self.queue = asyncio.Queue()
        self.fatal = None
        self.allowed_flags = None
        self.transitions = []

    def log(self, event: str, **values) -> dict:
        self.seq += 1
        record = {"seq": self.seq, "event": event,
                  "utc": datetime.now(timezone.utc).isoformat(),
                  "monotonic": asyncio.get_running_loop().time(), **values}
        self.file.write(json.dumps(record) + "\n")
        self.file.flush()
        if event != "rx":
            print(json.dumps(record), flush=True)
        return record

    def callback(self, _characteristic, data):
        raw = bytes(data)
        now = asyncio.get_running_loop().time()
        try:
            status = protocol.decode_status(raw, now)
            if status.flags not in CANDIDATES:
                raise ValueError("unknown profile or lost preserved RX 04/08")
            if self.allowed_flags is not None and status.flags not in self.allowed_flags:
                raise ValueError(f"unexpected status flags {status.flags:02x}")
            self.latest = self.log("rx", frame=raw.hex(), status=protocol.status_payload(status))
            self.queue.put_nowait(self.latest)
        except ValueError as error:
            self.latest = None
            self.fatal = str(error)
            self.queue.put_nowait(self.log("rx_invalid", frame=raw.hex(), reason=str(error)))

    async def fresh(self, after: float) -> dict:
        deadline = asyncio.get_running_loop().time() + 3
        while True:
            if self.fatal:
                raise RuntimeError(self.fatal)
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                raise TimeoutError("no fresh valid status within 3 seconds")
            record = await asyncio.wait_for(self.queue.get(), remaining)
            now = asyncio.get_running_loop().time()
            if record["event"] == "rx_invalid":
                raise RuntimeError(record["reason"])
            if after < record["monotonic"] <= now and now - record["monotonic"] <= 3:
                return record

    async def photo(self, label: str):
        # Existing camera infrastructure only; no configuration or rebuilding.
        script = Path("/tmp/allpowers-camera/snapshot.sh")
        if not script.exists() or not Path("/dev/video0").exists():
            return self.log("photo_unavailable", label=label)
        process = await asyncio.create_subprocess_exec(
            str(script), stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            output, error = await asyncio.wait_for(process.communicate(), 5)
        except TimeoutError:
            process.kill()
            await process.communicate()
            return self.log("photo_unavailable", label=label, reason="5-second capture bound")
        path = Path(output.decode().strip())
        if process.returncode or not path.is_file():
            return self.log("photo_unavailable", label=label, reason=error.decode())
        return self.log("photo", label=label, path=str(path),
                        sha256=hashlib.sha256(path.read_bytes()).hexdigest())

    async def transition(self, client, source: int, target: int, *, restoration=False):
        if (source ^ target).bit_count() != 1:
            raise ValueError("each transition must change exactly one logical switch")
        before = await self.fresh(asyncio.get_running_loop().time())
        if before["status"]["flags"] != source:
            raise RuntimeError("pre-write status differs from the last confirmed state")
        frame = candidate_frame(target)
        if (source & 0x0C) != (target & 0x0C):
            raise RuntimeError("preserved fields would change")
        protocol.verify_device(self.device)
        if (not client.is_connected or self.fatal or self.latest != before
                or asyncio.get_running_loop().time() - before["monotonic"] > 3):
            raise RuntimeError("write refused: disconnected, changed, or stale status")
        tx = self.log("tx", before=before, source_state=state(source), target_state=state(target),
                      expected_rx_flags=target, tx_flags=frame[7], frame=frame.hex(),
                      checksum=frame[8], response=False, restoration=restoration)
        # One write, no retry. The sequential caller owns the entire confirmation window.
        self.allowed_flags = {source, target}
        await asyncio.wait_for(client.write_gatt_char(protocol.CONTROL_UUID, frame, response=False), 3)
        matches = []
        deadline = asyncio.get_running_loop().time() + 10
        after = tx["monotonic"]
        while len(matches) < 2:
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError("two complete-state confirmations missing")
            record = await self.fresh(after)
            after = record["monotonic"]
            flags = record["status"]["flags"]
            if flags == target:
                matches.append(record)
                self.allowed_flags = {target}
            elif flags != source or matches:
                raise RuntimeError(f"unexpected post-write flags {flags:02x}")
            if not client.is_connected:
                raise RuntimeError("disconnected during confirmation")
        vector = {"tx_seq": tx["seq"], "source_state": state(source), "starting_status": before,
                  "target_state": state(target), "expected_rx_flags": target,
                  "tx_flags": frame[7], "tx_frame": frame.hex(), "checksum": frame[-1],
                  "first_two_post_write_status_frames": matches,
                  "first_response_latency_s": matches[0]["monotonic"] - tx["monotonic"],
                  "second_confirmation_latency_s": matches[1]["monotonic"] - tx["monotonic"],
                  "unrelated_switches_preserved": True, "restoration": restoration,
                  "physical_observation": {"verification": "PENDING_IMAGE_REVIEW"}}
        self.transitions.append(vector)
        self.log("confirmed", tx_seq=tx["seq"], flags=target)
        vector["physical_observation"]["capture"] = await self.photo(f"rx-{target:02x}")


async def run(directory: Path):
    capture = Capture(directory)
    original = None
    current = None
    primary = None
    try:
        adapter = await protocol.resolve_adapter()
        capture.log("adapter", mac=protocol.ADAPTER_ADDRESS, hci=adapter)
        device = await BleakScanner.find_device_by_address(
            protocol.STATION_ADDRESS, timeout=20, bluez={"adapter": adapter})
        if device is None:
            raise RuntimeError("exact S300 not discovered; no writes")
        protocol.verify_device(device)
        capture.device = device
        capture.log("device", address=device.address, name=device.name)
        async with BleakClient(device, timeout=25, bluez={"adapter": adapter}) as client:
            service = client.services.get_service(protocol.SERVICE_UUID)
            if service is None:
                raise RuntimeError("fff0 missing")
            notify = service.get_characteristic(protocol.STATUS_UUID)
            write = service.get_characteristic(protocol.CONTROL_UUID)
            if notify is None or "notify" not in notify.properties:
                raise RuntimeError("fff1 notify missing")
            if write is None or "write-without-response" not in write.properties:
                raise RuntimeError("fff2 write-without-response missing")
            name = bytes(await client.read_gatt_char(protocol.NAME_UUID)).decode().rstrip("\x00")
            if name != protocol.STATION_NAME:
                raise RuntimeError("readable station identity mismatch")
            capture.log("gatt_verified", service=service.uuid, notify=notify.uuid,
                        write=write.uuid, name=name, write_properties=write.properties)
            await client.start_notify(protocol.STATUS_UUID, capture.callback)
            try:
                original = await capture.fresh(asyncio.get_running_loop().time())
                current = original["status"]["flags"]
                if current not in PRIOR_RX:
                    raise RuntimeError("initial profile was not previously qualified; no writes")
                capture.log("initial", status=original, state=state(current))
                capture.allowed_flags = {current}
                await capture.photo("initial")
                offset = GRAY_RX.index(current)
                path = GRAY_RX[offset:] + GRAY_RX[:offset] + (current,)
                for target in path[1:]:
                    await capture.transition(client, current, target)
                    current = target
                final = await capture.fresh(asyncio.get_running_loop().time())
                if final["status"]["flags"] != original["status"]["flags"]:
                    raise RuntimeError("final status does not equal original")
                capture.log("restored", status=final, state=state(current))
            except BaseException as error:
                primary = error
                capture.log("stopped", type=type(error).__name__, reason=str(error))
                # Guarded restoration uses only the original previously verified frame.
                # It is not a retry of the failed command; unknown/unconfirmed state stops.
                latest = capture.latest
                if (original is not None and latest is not None and not capture.fatal
                        and client.is_connected
                        and latest["status"]["flags"] == current
                        and current != original["status"]["flags"]
                        and (current ^ original["status"]["flags"]).bit_count() == 1):
                    try:
                        await asyncio.wait_for(capture.transition(
                            client, current, original["status"]["flags"], restoration=True), 12)
                        capture.log("restored_after_stop", status=capture.latest)
                    except BaseException as restore_error:
                        capture.log("manual_restore_required", reason=str(restore_error),
                                    original=original, latest=capture.latest)
                elif original is not None and (latest is None or latest["status"]["flags"]
                                              != original["status"]["flags"]):
                    capture.log("manual_restore_required", original=original, latest=latest)
            finally:
                await client.stop_notify(protocol.STATUS_UUID)
        capture.log("disconnected", connected=client.is_connected)
    except BaseException as error:
        if primary is None:
            primary = error
        capture.log("error", type=type(error).__name__, reason=str(error))
    finally:
        capture.log("closed", success=primary is None, final_status=capture.latest)
        capture.file.close()
        result = {"identity": {"station": protocol.STATION_NAME, "address": protocol.STATION_ADDRESS,
                               "adapter_mac": protocol.ADAPTER_ADDRESS},
                  "initial_status": original, "transitions": capture.transitions,
                  "final_status": capture.latest, "success": primary is None,
                  "error": str(primary) if primary else None}
        (directory / "matrix_vectors.json").write_text(json.dumps(result, indent=2) + "\n")
    if primary:
        raise primary


if __name__ == "__main__":
    # Explicit directory required; existing evidence can never be overwritten.
    if len(sys.argv) != 2:
        raise SystemExit("usage: uv run --no-project s300_matrix_validation.py NEW_CAPTURE_DIRECTORY")
    asyncio.run(run(Path(sys.argv[1])))
