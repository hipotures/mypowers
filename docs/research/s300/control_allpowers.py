# /// script
# requires-python = ">=3.12"
# dependencies = ["bleak==3.0.2", "dbus-fast==3.1.2"]
# ///
"""Bounded, status-confirmed control research for the verified S300 profile."""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import sys
from dataclasses import dataclass
from functools import reduce
from operator import xor
from typing import Any

from bleak import BleakClient, BleakScanner
from bleak.exc import BleakError
from dbus_fast import BusType, Message, MessageType
from dbus_fast.aio import MessageBus

STATION_ADDRESS = "2A:02:01:48:6B:D0"
STATION_NAME = "AP S300 V2.0"
ADAPTER_ADDRESS = "F4:4E:FC:A1:CB:FF"
SERVICE_UUID = "0000fff0-0000-1000-8000-00805f9b34fb"
STATUS_UUID = "0000fff1-0000-1000-8000-00805f9b34fb"
CONTROL_UUID = "0000fff2-0000-1000-8000-00805f9b34fb"
NAME_UUID = "00002a00-0000-1000-8000-00805f9b34fb"

STATUS_DC, STATUS_AC, STATUS_FREQUENCY = 0x01, 0x02, 0x04
STATUS_BEEP, STATUS_LIGHT, STATUS_SCREEN, STATUS_VOICE = 0x08, 0x10, 0x20, 0x40
CONTROL_DC, CONTROL_AC, CONTROL_FREQUENCY = 0x01, 0x02, 0x08
CONTROL_BEEP, CONTROL_LIGHT, CONTROL_SCREEN, CONTROL_VOICE = 0x10, 0x20, 0x40, 0x80
CONTROL_PREFIX = bytes((0xA5, 0x65, 0x00, 0xB1, 0x01, 0x01, 0x00))
# All eight complete AC/DC/common-lamp states qualified by the matrix capture.
# RX04/RX08 remain set; all higher fields remain clear. No other TX is permitted.
CONTROL_ALLOWLIST = frozenset((0x18, 0x19, 0x1A, 0x1B, 0x38, 0x39, 0x3A, 0x3B))
STATUS_PROFILES = frozenset((0x0C, 0x0D, 0x0E, 0x0F, 0x1C, 0x1D, 0x1E, 0x1F))

SCAN_TIMEOUT = 20.0
CLIENT_TIMEOUT = 25.0
STATUS_FRESHNESS = 3.0
CONFIRMATION_TIMEOUT = 10.0
CLEANUP_TIMEOUT = 5.0


@dataclass(frozen=True, slots=True)
class Status:
    flags: int
    battery_percent: int
    input_power_w: int
    output_power_w: int
    remaining_minutes: int
    received_at: float


@dataclass(frozen=True, slots=True)
class Notification:
    status: Status | None
    received_at: float


def emit(payload: dict[str, Any], *, error: bool = False) -> None:
    print(json.dumps(payload, separators=(",", ":")), file=sys.stderr if error else sys.stdout, flush=True)


def status_payload(status: Status) -> dict[str, Any]:
    f = status.flags
    return {
        "flags": f,
        "dc_enabled": bool(f & STATUS_DC), "ac_enabled": bool(f & STATUS_AC),
        "frequency_flag": bool(f & STATUS_FREQUENCY), "unverified_beep_flag": bool(f & STATUS_BEEP),
        "light_enabled": bool(f & STATUS_LIGHT), "unverified_screen_flag": bool(f & STATUS_SCREEN),
        "unverified_voice_flag": bool(f & STATUS_VOICE), "battery_percent": status.battery_percent,
        "input_power_w": status.input_power_w, "output_power_w": status.output_power_w,
        "remaining_minutes": status.remaining_minutes,
    }


def decode_status(frame: bytes, received_at: float = 0.0) -> Status:
    if len(frame) != 16 or frame[:2] != bytes((0xA5, 0x65)):
        raise ValueError("invalid status header or length")
    if frame[2:5] != bytes((0xB1, 0x00, 0x01)):
        raise ValueError("unexpected status envelope")
    if frame[5] != 0x08 or len(frame) != 8 + frame[5]:
        raise ValueError("invalid status length byte")
    if frame[6] != 0x01:
        raise ValueError("unexpected status command")
    if reduce(xor, frame, 0) != 0:
        raise ValueError("invalid status checksum")
    if frame[8] > 100:
        raise ValueError("invalid battery percentage")
    flags = frame[7]
    if flags & ~0x7F:
        raise ValueError("unknown higher status flag bits")
    return Status(
        flags, frame[8], int.from_bytes(frame[9:11], "big"), int.from_bytes(frame[11:13], "big"),
        int.from_bytes(frame[13:15], "big"), received_at,
    )


def expected_status_flags(status_flags: int, field: str, enabled: bool) -> int:
    if status_flags not in STATUS_PROFILES:
        raise ValueError("status flags are outside the verified exercise profiles")
    mask = {"ac": STATUS_AC, "dc": STATUS_DC, "light": STATUS_LIGHT}.get(field)
    if mask is None:
        raise ValueError(f"unsupported exercise field: {field}")
    result = (status_flags | mask) if enabled else (status_flags & ~mask)
    if result not in STATUS_PROFILES:
        raise ValueError("requested state is outside the verified status profiles")
    return result


def encode_control_flags(
    status_flags: int, *, ac: bool | None = None, dc: bool | None = None, light: bool | None = None
) -> int:
    """Encode a complete vendor RMW flag byte, overriding one supported output."""
    if status_flags not in STATUS_PROFILES:
        raise ValueError("status flags are outside the verified exercise profiles")
    mapping = (
        (STATUS_DC, CONTROL_DC), (STATUS_AC, CONTROL_AC), (STATUS_FREQUENCY, CONTROL_FREQUENCY),
        (STATUS_BEEP, CONTROL_BEEP), (STATUS_LIGHT, CONTROL_LIGHT),
        (STATUS_SCREEN, CONTROL_SCREEN), (STATUS_VOICE, CONTROL_VOICE),
    )
    result = reduce(lambda value, pair: value | (pair[1] if status_flags & pair[0] else 0), mapping, 0)
    for value, mask in ((ac, CONTROL_AC), (dc, CONTROL_DC), (light, CONTROL_LIGHT)):
        if value is not None:
            result = (result | mask) if value else (result & ~mask)
    if result not in CONTROL_ALLOWLIST:
        raise ValueError("encoded control flags are outside the verified allowlist")
    return result


def control_frame(control_flags: int) -> bytes:
    if control_flags not in CONTROL_ALLOWLIST:
        raise ValueError("control flags are outside the verified allowlist")
    body = CONTROL_PREFIX + bytes((control_flags,))
    return body + bytes((reduce(xor, body, 0),))


def _plain(value: Any) -> Any:
    return getattr(value, "value", value)


async def resolve_adapter() -> str:
    """Resolve the current hciN for the Actions adapter without a fallback."""
    bus: MessageBus | None = None
    try:
        bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        reply = await bus.call(Message(
            destination="org.bluez", path="/", interface="org.freedesktop.DBus.ObjectManager",
            member="GetManagedObjects",
        ))
        if reply.message_type == MessageType.ERROR:
            raise RuntimeError(f"BlueZ GetManagedObjects failed: {reply.error_name}")
        matches = []
        for path, interfaces in (reply.body[0] if reply.body else {}).items():
            props = interfaces.get("org.bluez.Adapter1")
            if props is None or str(_plain(props.get("Address", ""))).upper() != ADAPTER_ADDRESS:
                continue
            name = str(path).rsplit("/", 1)[-1]
            if not name.startswith("hci") or not name[3:].isdigit():
                raise RuntimeError(f"invalid BlueZ adapter path: {path}")
            if _plain(props.get("Powered", False)) is not True:
                raise RuntimeError("Actions adapter is not powered")
            matches.append(name)
        if len(matches) != 1:
            raise RuntimeError(f"expected one powered Actions adapter, found {len(matches)}")
        return matches[0]
    finally:
        if bus is not None:
            bus.disconnect()


def verify_device(device: Any) -> None:
    address, name = str(getattr(device, "address", "")).upper(), getattr(device, "name", None)
    if address != STATION_ADDRESS or name != STATION_NAME:
        raise RuntimeError(f"discovered device identity mismatch: address={address!r}, name={name!r}")


class StatusStream:
    def __init__(self, debug: bool) -> None:
        self.debug, self.latest = debug, None
        self.queue: asyncio.Queue[Notification] = asyncio.Queue()

    def callback(self, _characteristic: Any, data: bytearray) -> None:
        received_at = asyncio.get_running_loop().time()
        try:
            status = decode_status(bytes(data), received_at)
        except ValueError as error:
            self.latest = None
            self.queue.put_nowait(Notification(None, received_at))
            if self.debug:
                emit({"event": "rx_invalid", "frame": bytes(data).hex(), "reason": str(error)})
            return
        self.latest = status
        self.queue.put_nowait(Notification(status, received_at))
        if self.debug:
            emit({"event": "rx", "frame": bytes(data).hex(), "state": status_payload(status)})
        else:
            emit({"event": "status", **status_payload(status)})


async def wait_fresh(stream: StatusStream, after: float, timeout: float = STATUS_FRESHNESS) -> Status:
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        now, latest = asyncio.get_running_loop().time(), stream.latest
        if (
            latest is not None and latest.received_at >= after
            and latest.received_at <= now and now - latest.received_at <= STATUS_FRESHNESS
        ):
            return latest
        remaining = deadline - now
        if remaining <= 0:
            raise TimeoutError("no fresh status notification within three seconds")
        try:
            notification = await asyncio.wait_for(stream.queue.get(), remaining)
        except TimeoutError as error:
            raise TimeoutError("no fresh status notification within three seconds") from error
        now = asyncio.get_running_loop().time()
        status = notification.status
        if (
            status is not None and status.received_at >= after
            and status.received_at <= now and now - status.received_at <= STATUS_FRESHNESS
        ):
            return status


async def wait_matches(stream: StatusStream, expected: int, after: float) -> Status:
    if expected not in STATUS_PROFILES:
        raise ValueError("confirmation flags are outside the verified profiles")
    deadline, matches = asyncio.get_running_loop().time() + CONFIRMATION_TIMEOUT, 0
    while True:
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining <= 0:
            raise TimeoutError(f"did not receive two confirmations for flags {expected}")
        try:
            notification = await asyncio.wait_for(stream.queue.get(), remaining)
        except TimeoutError as error:
            raise TimeoutError(f"did not receive two confirmations for flags {expected}") from error
        status = notification.status
        now = asyncio.get_running_loop().time()
        if notification.received_at < after:
            continue
        if notification.received_at > now or now - notification.received_at > STATUS_FRESHNESS:
            matches = 0
            continue
        if status is not None and status.flags == expected:
            matches += 1
            if matches == 2:
                return status
        else:
            matches = 0


WRITE_LOCK = asyncio.Lock()


async def send_control(
    client: BleakClient, device: Any, stream: StatusStream, control_flags: int, expected_flags: int
) -> float:
    frame = control_frame(control_flags)
    async with WRITE_LOCK:
        verify_device(device)
        if not client.is_connected:
            raise RuntimeError("write refused: Bluetooth link is disconnected")
        current = stream.latest
        now = asyncio.get_running_loop().time()
        if (
            current is None
            or current.flags != expected_flags
            or current.received_at > now
            or now - current.received_at > STATUS_FRESHNESS
        ):
            raise RuntimeError("write refused: status is not fresh and exactly expected")
        sent = asyncio.get_running_loop().time()
        payload = {"event": "tx", "flags": control_flags, "checksum": frame[-1], "response": False}
        if stream.debug:
            payload["frame"] = frame.hex()
        emit(payload)
        await client.write_gatt_char(CONTROL_UUID, frame, response=False)
    return sent


async def restore_exact(
    client: BleakClient, device: Any, stream: StatusStream, original: Status, expected_test: int
) -> None:
    if not client.is_connected:
        raise RuntimeError("manual restore required: Bluetooth link is disconnected")
    current = await wait_fresh(stream, asyncio.get_running_loop().time())
    if current.flags not in (expected_test, original.flags):
        raise RuntimeError(f"manual restore required: current status flags are {current.flags}")
    emit({"event": "restore_before", **status_payload(current)})
    if current.flags == original.flags:
        emit({"event": "restored", "flags": original.flags, "write": False})
        return
    sent = await send_control(client, device, stream, encode_control_flags(original.flags), current.flags)
    confirmed = await wait_matches(stream, original.flags, sent)
    emit({"event": "restored", "flags": confirmed.flags, "write": True})


def report_manual(original: Status, expected_test: int, reason: str) -> None:
    emit({
        "event": "manual_restore_required", "reason": reason,
        "original_status": status_payload(original), "expected_test_flags": expected_test,
    }, error=True)


async def run_probe(args: argparse.Namespace) -> None:
    adapter = await resolve_adapter()
    emit({"event": "adapter", "address": ADAPTER_ADDRESS, "hci": adapter})
    device = await BleakScanner.find_device_by_address(
        STATION_ADDRESS, timeout=SCAN_TIMEOUT, bluez={"adapter": adapter}
    )
    if device is None:
        raise RuntimeError("station was not discovered on the resolved Actions adapter")
    verify_device(device)
    emit({"event": "device", "address": STATION_ADDRESS, "name": STATION_NAME})
    async with BleakClient(device, timeout=CLIENT_TIMEOUT, bluez={"adapter": adapter}) as client:
        verify_device(device)
        service = client.services.get_service(SERVICE_UUID)
        if service is None:
            raise RuntimeError("station does not expose the expected fff0 service")
        status_char = service.get_characteristic(STATUS_UUID)
        control_char = service.get_characteristic(CONTROL_UUID)
        if status_char is None or "notify" not in status_char.properties:
            raise RuntimeError("fff1 is not a notify characteristic")
        if control_char is None or "write-without-response" not in control_char.properties:
            raise RuntimeError("fff2 is not a write-without-response characteristic")
        name_char = client.services.get_characteristic(NAME_UUID)
        if name_char is None or "read" not in name_char.properties:
            raise RuntimeError("station does not expose a readable 2a00 name characteristic")
        try:
            actual_name = bytes(await client.read_gatt_char(NAME_UUID)).decode("utf-8").rstrip("\x00")
        except (UnicodeDecodeError, BleakError) as error:
            raise RuntimeError("station 2a00 name could not be read") from error
        if actual_name != STATION_NAME:
            raise RuntimeError(f"GATT name mismatch: {actual_name!r}")
        stream = StatusStream(args.debug)
        started = target_written = restoration_attempted = restored = False
        original: Status | None = None
        expected: int | None = None
        primary: BaseException | None = None
        try:
            await client.start_notify(STATUS_UUID, stream.callback)
            started = True
            original = await wait_fresh(stream, asyncio.get_running_loop().time())
            if args.exercise is None:
                await asyncio.sleep(args.seconds)
            else:
                if original.flags not in STATUS_PROFILES:
                    raise RuntimeError(f"exercise refuses unknown initial status flags {original.flags}")
                before = await wait_fresh(stream, asyncio.get_running_loop().time())
                if before.flags != original.flags:
                    raise RuntimeError("write refused: fresh pre-write status differs from the original")
                original = before
                mask = {"ac": STATUS_AC, "dc": STATUS_DC, "light": STATUS_LIGHT}[args.exercise]
                enabled = not bool(before.flags & mask)
                expected = expected_status_flags(before.flags, args.exercise, enabled)
                control_flags = encode_control_flags(before.flags, **{args.exercise: enabled})
                emit({"event": "before", **status_payload(original)})
                target_written = True
                sent = await send_control(client, device, stream, control_flags, before.flags)
                confirmed = await wait_matches(stream, expected, sent)
                emit({"event": "status_confirmed", "phase": "exercise", "flags": confirmed.flags})
                await asyncio.sleep(args.hold_seconds)
                restoration_attempted = True
                await restore_exact(client, device, stream, original, expected)
                restored = True
        except (Exception, asyncio.CancelledError) as error:  # noqa: BLE001
            primary = error
            if original is not None and expected is not None and target_written:
                if not restoration_attempted:
                    restoration_attempted = True
                    try:
                        await asyncio.wait_for(restore_exact(client, device, stream, original, expected), CLEANUP_TIMEOUT)
                        restored = True
                    except (Exception, asyncio.CancelledError) as cleanup_error:  # noqa: BLE001
                        report_manual(original, expected, str(cleanup_error))
                elif not restored:
                    report_manual(original, expected, str(error))
        finally:
            if started:
                try:
                    await client.stop_notify(STATUS_UUID)
                except (Exception, asyncio.CancelledError) as cleanup_error:  # noqa: BLE001
                    emit({"event": "cleanup_error", "reason": str(cleanup_error)}, error=True)
                    if primary is None:
                        primary = cleanup_error
        if primary is not None:
            raise primary


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exercise", choices=("ac", "dc", "light"))
    parser.add_argument("--seconds", type=float, default=5.0)
    parser.add_argument("--hold-seconds", type=float, default=3.0)
    parser.add_argument("--overall-timeout", type=float, default=60.0)
    parser.add_argument("--debug", action="store_true")
    args = parser.parse_args()
    if (
        not math.isfinite(args.seconds) or not 0 <= args.seconds <= 300
        or not math.isfinite(args.hold_seconds) or not 0 <= args.hold_seconds <= 10
        or not math.isfinite(args.overall_timeout)
        or not SCAN_TIMEOUT + CLIENT_TIMEOUT < args.overall_timeout <= 360
    ):
        parser.error("durations must be finite; seconds 0..300, hold-seconds 0..10, overall-timeout 45..360")
    return args


def main() -> int:
    args = parse_args()
    try:
        asyncio.run(asyncio.wait_for(run_probe(args), args.overall_timeout))
    except KeyboardInterrupt:
        emit({"event": "cancelled"}, error=True)
        return 130
    except (BleakError, RuntimeError, TimeoutError, ValueError) as error:
        emit({"event": "error", "reason": str(error)}, error=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
