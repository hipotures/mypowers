# /// script
# requires-python = ">=3.12"
# dependencies = ["bleak==3.0.2"]
# ///
"""Read validated S300 BLE status notifications without changing outputs."""

import argparse
import asyncio
from datetime import datetime, timezone
from functools import reduce
import json
from operator import xor

from bleak import BleakClient, BleakScanner
from bleak.exc import BleakError

SERVICE = "0000fff0-0000-1000-8000-00805f9b34fb"
STATUS = "0000fff1-0000-1000-8000-00805f9b34fb"


def decode_status(frame: bytes) -> dict:
    """Decode a complete status frame with verified length and XOR checksum."""
    if len(frame) < 16 or frame[:2] != b"\xa5\x65":
        raise ValueError("Invalid status header or truncated frame")
    if len(frame) != 8 + frame[5] or reduce(xor, frame, 0) != 0:
        raise ValueError("Invalid status length or checksum")
    if frame[6] != 1 or frame[8] > 100:
        raise ValueError("Unexpected command or invalid battery percentage")
    return {
        "battery_percent": frame[8],
        "input_power_w": int.from_bytes(frame[9:11], "big"),
        "output_power_w": int.from_bytes(frame[11:13], "big"),
        "remaining_minutes": int.from_bytes(frame[13:15], "big"),
        "ac_enabled": bool(frame[7] & 2),
        "dc_enabled": bool(frame[7] & 1),
        "light_enabled": bool(frame[7] & 16),
        "raw_flags": frame[7],
        "raw_frame": frame.hex(),
    }


async def read_station(args: argparse.Namespace) -> None:
    device = await BleakScanner.find_device_by_address(
        args.address, timeout=20, adapter=args.adapter
    )
    if device is None:
        raise RuntimeError("Station not found; check station Bluetooth and phone connection")
    received = 0

    def notification(_characteristic, data: bytearray) -> None:
        nonlocal received
        try:
            result = decode_status(bytes(data))
        except ValueError as error:
            print(json.dumps({"invalid_notification": str(error), "raw_frame": data.hex()}), flush=True)
            return
        received += 1
        result["timestamp"] = datetime.now(timezone.utc).isoformat()
        print(json.dumps(result), flush=True)

    async with BleakClient(device, timeout=25, bluez={"adapter": args.adapter}) as client:
        service = client.services.get_service(SERVICE)
        if service is None or not any(c.uuid == STATUS for c in service.characteristics):
            raise RuntimeError("Station does not expose the expected S300 status characteristic")
        await client.start_notify(STATUS, notification)
        await asyncio.sleep(args.seconds)
        if not client.is_connected:
            raise RuntimeError("Station disconnected during monitoring")
        await client.stop_notify(STATUS)
    if received == 0:
        raise RuntimeError("No validated status notifications received")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="2A:02:01:48:6B:D0")
    parser.add_argument("--adapter", default="hci2", help="BlueZ adapter; Actions was verified as hci2")
    parser.add_argument("--seconds", type=int, default=10)
    args = parser.parse_args()
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    try:
        asyncio.run(read_station(args))
    except (BleakError, RuntimeError, TimeoutError) as error:
        parser.exit(1, f"Read failed: {error}\n")
    except KeyboardInterrupt:
        parser.exit(130)
