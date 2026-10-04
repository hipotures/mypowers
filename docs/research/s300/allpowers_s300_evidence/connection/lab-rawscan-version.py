# /// script
# requires-python = ">=3.12"
# dependencies = ["bleak==3.0.2", "dbus-fast==3.1.2"]
# ///
"""Supervised, read-only S300 connection experiments; not a daemon or retry loop.

Commands: scan [hciN] [seconds], rawscan hciN [seconds], connect [fff1|ff03|none], observe [seconds],
disconnect, snapshot, note <operator observation>, quit. Run only one instance.
No characteristic writes, pairing, adapter mutations, or service mutations.
"""

import asyncio
from datetime import datetime, timezone
import importlib.metadata
import json
from pathlib import Path
import sys
import traceback

from bleak import BleakClient, BleakScanner
from dbus_fast import BusType, Message, MessageType
from dbus_fast.aio import MessageBus

from control_allpowers import (
    ADAPTER_ADDRESS, STATION_ADDRESS, STATUS_UUID, decode_status, resolve_adapter,
)


class Lab:
    def __init__(self, path):
        self.file = Path(path).open("x", encoding="utf-8")
        self.origin = asyncio.get_running_loop().time()
        self.device = None
        self.client = None
        self.adapter = None
        self.received = 0
        self.valid = 0
        self.subscribed = None
        self.command = None
        self.sequence = 0

    def log(self, event, **data):
        self.sequence += 1
        record = {
            "seq": self.sequence, "utc": datetime.now(timezone.utc).isoformat(),
            "elapsed_s": round(asyncio.get_running_loop().time() - self.origin, 6),
            "command": self.command, "event": event, **data,
        }
        line = json.dumps(record, default=str)
        self.file.write(line + "\n")
        self.file.flush()
        if event != "rx" or self.received % 10 == 1:
            print(line, flush=True)

    async def snapshot(self):
        bus = None
        try:
            bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
            reply = await bus.call(Message(
                destination="org.bluez", path="/",
                interface="org.freedesktop.DBus.ObjectManager", member="GetManagedObjects",
            ))
            if reply.message_type == MessageType.ERROR:
                self.log("bluez_error", name=reply.error_name, body=reply.body)
                return
            objects = {}
            for path, interfaces in reply.body[0].items():
                for interface in ("org.bluez.Adapter1", "org.bluez.Device1"):
                    props = interfaces.get(interface)
                    if not props:
                        continue
                    fields = {k: v.value for k, v in props.items() if k in (
                        "Address", "AddressType", "Name", "Powered", "Discovering",
                        "Connected", "ServicesResolved", "RSSI", "Paired", "Trusted",
                    )}
                    if interface.endswith("Adapter1") or fields.get("Address") == STATION_ADDRESS:
                        objects[path] = fields
            self.log("bluez_snapshot", objects=objects)
        finally:
            if bus is not None:
                bus.disconnect()

    async def scan(self, adapter, seconds):
        count = 0
        others = set()
        started = asyncio.get_running_loop().time()

        def callback(device, advertisement):
            nonlocal count
            if device.address.upper() != STATION_ADDRESS:
                others.add(device.address)
                return
            count += 1
            if adapter == self.adapter:
                self.device = device
            self.log("scan_callback", adapter=adapter, since_scan_s=round(
                asyncio.get_running_loop().time() - started, 6), address=device.address,
                name=device.name, local_name=advertisement.local_name,
                rssi=advertisement.rssi, manufacturer={str(k): v.hex() for k, v in
                    advertisement.manufacturer_data.items()}, details=device.details)

        self.log("scan_start", adapter=adapter, seconds=seconds)
        async with BleakScanner(callback, bluez={"adapter": adapter}):
            await asyncio.sleep(seconds)
        self.log("scan_end", adapter=adapter, station_callbacks=count,
                 other_addresses=len(others), duration_s=asyncio.get_running_loop().time() - started)

    def notification(self, characteristic, data):
        self.received += 1
        record = {"uuid": characteristic.uuid, "frame": bytes(data).hex()}
        try:
            status = decode_status(bytes(data))
            self.valid += 1
            record["state"] = {
                "flags": status.flags, "battery": status.battery_percent,
                "input_w": status.input_power_w, "output_w": status.output_power_w,
                "minutes": status.remaining_minutes,
            }
        except ValueError as error:
            record["invalid"] = str(error)
        self.log("rx", **record)

    async def disconnect(self):
        if self.client is None:
            return
        try:
            self.log("disconnect_start", connected=self.client.is_connected)
            await asyncio.wait_for(self.client.disconnect(), 10)
            self.log("disconnect_complete", connected=self.client.is_connected)
        finally:
            self.client = None
            self.subscribed = None

    async def connect(self, channel):
        if self.client is not None:
            raise RuntimeError("Disconnect the current experiment before connecting again")
        if self.device is None:
            raise RuntimeError("No cached device from a prior Actions scan")
        self.received = self.valid = 0
        self.client = BleakClient(
            self.device, timeout=25, bluez={"adapter": self.adapter},
            disconnected_callback=lambda client: self.log("disconnected_callback"),
        )
        self.log("connect_start", device=self.device.address, cached_details=self.device.details)
        await asyncio.wait_for(self.client.connect(), 35)
        self.log("gatt_connected", connected=self.client.is_connected,
                 services=[s.uuid for s in self.client.services])
        if channel != "none":
            uuid = STATUS_UUID if channel == "fff1" else "0000ff03-0000-1000-8000-00805f9b34fb"
            await self.client.start_notify(uuid, self.notification)
            self.subscribed = uuid
            self.log("subscribed", uuid=uuid)

    async def observe(self, seconds):
        before, valid_before = self.received, self.valid
        self.log("observe_start", seconds=seconds)
        await asyncio.sleep(seconds)
        connected = self.client is not None and self.client.is_connected
        self.log("observe_end", connected=connected, notifications=self.received - before,
                 valid_status=self.valid - valid_before)
        if connected and self.valid == valid_before:
            raise TimeoutError(f"no validated telemetry during {seconds:g}-second observation")

    async def command_run(self, line):
        self.command = line
        started = asyncio.get_running_loop().time()
        self.log("command_start")
        try:
            words = line.split()
            match words[0]:
                case "scan" | "rawscan":
                    if words[0] == "scan" and self.adapter is None:
                        self.adapter = await resolve_adapter()
                    await self.scan(words[1] if len(words) > 1 else self.adapter,
                                    float(words[2]) if len(words) > 2 else 20)
                case "connect":
                    channel = words[1] if len(words) > 1 else "fff1"
                    if channel not in ("fff1", "ff03", "none"):
                        raise ValueError("unsupported observation channel")
                    await self.connect(channel)
                case "observe":
                    await self.observe(float(words[1]) if len(words) > 1 else 5)
                case "disconnect":
                    await self.disconnect()
                case "snapshot":
                    await self.snapshot()
                case "resolve":
                    self.adapter = await resolve_adapter()
                    self.log("resolved", adapter=self.adapter)
                case "note":
                    self.log("operator_note", text=line.partition(" ")[2])
                case "quit":
                    return False
                case _:
                    raise ValueError("unknown research command")
        except Exception as error:
            self.log("exception", type=f"{type(error).__module__}.{type(error).__qualname__}",
                     message=str(error), repr=repr(error), args=error.args,
                     dbus_error=getattr(error, "dbus_error", None),
                     dbus_details=getattr(error, "dbus_error_details", None),
                     traceback=traceback.format_exc(), duration_s=asyncio.get_running_loop().time() - started)
            if words[0] == "connect":
                await self.disconnect()
        self.log("command_end", duration_s=asyncio.get_running_loop().time() - started)
        return True


async def main():
    lab = Lab(sys.argv[1])
    lab.log("environment", python=sys.version, bleak=importlib.metadata.version("bleak"),
            dbus_fast=importlib.metadata.version("dbus-fast"), address=STATION_ADDRESS,
            adapter_mac=ADAPTER_ADDRESS)
    reader = asyncio.StreamReader()
    transport, _ = await asyncio.get_running_loop().connect_read_pipe(
        lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
    try:
        while line := await reader.readline():
            line = line.decode().strip()
            if line and not await lab.command_run(line):
                break
    finally:
        try:
            await lab.disconnect()
        finally:
            transport.close()
            lab.log("lab_closed")
            lab.file.close()


if __name__ == "__main__":
    asyncio.run(main())
