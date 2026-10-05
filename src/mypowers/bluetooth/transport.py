"""Explicit BlueZ adapter resolution and the exact-unit Bleak transport."""

import asyncio
import logging
from collections.abc import Callable
from pathlib import Path
from typing import Any, Protocol

from bleak import BleakClient, BleakScanner
from dbus_fast import BusType, Message, MessageType
from dbus_fast.aio import MessageBus

from mypowers.config.server import ServerConfig
from mypowers.contracts import AppError
from mypowers.protocol import (
    CONTROL_UUID,
    NAME_UUID,
    PROFILE,
    SERVICE_UUID,
    STATION_ADDRESS,
    STATION_NAME,
    STATUS_UUID,
    validate_control,
)


class Transport(Protocol):
    adapter_id: str

    async def acquire(self, phase: Callable[[str], None]) -> None: ...
    async def subscribe(self, callback: Callable[[bytes], None]) -> None: ...
    async def write(self, frame: bytes) -> None: ...
    async def cleanup(self) -> None: ...
    def connected(self) -> bool: ...


def rfkill(adapter: str) -> bool | None:
    """Inspect only rfkill entries associated with the currently resolved controller."""
    root = Path("/sys/class/bluetooth") / adapter
    try:
        entries = list(root.glob("**/rfkill*"))
        if not entries:
            entries = [
                entry
                for entry in Path("/sys/class/rfkill").glob("rfkill*")
                if (entry / "name").read_text().strip() == adapter
            ]
        if not entries:
            return None
        return any(
            (entry / field).read_text().strip() == "1"
            for entry in entries
            for field in ("soft", "hard")
        )
    except OSError:
        return None


def classify_reply(reply: Message | None) -> None:
    if reply is None:
        raise AppError("system_bus_unavailable", "System bus did not answer.", 503)
    if reply.message_type == MessageType.ERROR:
        name = reply.error_name or ""
        if name in {
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
        }:
            raise AppError("bluez_unavailable", "BlueZ is unavailable; check its service.", 503)
        if name in {"org.freedesktop.DBus.Error.AccessDenied", "org.bluez.Error.NotAuthorized"}:
            raise AppError(
                "permission_denied", "BlueZ access denied; check account permissions.", 503
            )
        raise AppError("adapter_not_ready", "BlueZ could not inspect the configured adapter.", 503)


async def resolve_adapter(address: str) -> str:
    bus: MessageBus | None = None
    try:
        try:
            bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        except PermissionError:
            raise AppError("permission_denied", "System bus access denied.", 503) from None
        except (OSError, EOFError):
            raise AppError("system_bus_unavailable", "Cannot access the system bus.", 503) from None
        reply = await bus.call(
            Message(
                destination="org.bluez",
                path="/",
                interface="org.freedesktop.DBus.ObjectManager",
                member="GetManagedObjects",
            )
        )
        classify_reply(reply)
        assert reply is not None
        objects: dict[str, Any] = reply.body[0] if reply.body else {}
        for path, interfaces in objects.items():
            props = interfaces.get("org.bluez.Adapter1", {})
            if str(getattr(props.get("Address"), "value", "")).upper() != address.upper():
                continue
            adapter = path.rsplit("/", 1)[-1]
            if not adapter.startswith("hci") or not adapter[3:].isdigit():
                raise AppError("adapter_not_ready", "Invalid BlueZ adapter identifier.", 503)
            blocked = await asyncio.to_thread(rfkill, adapter)
            power = getattr(props.get("Powered"), "value", None)
            state = getattr(props.get("PowerState"), "value", None)
            if blocked is True or state == "off-blocked":
                raise AppError(
                    "adapter_blocked", "Configured Bluetooth controller is blocked.", 503
                )
            if power is not True:
                code = "adapter_off" if blocked is False else "adapter_not_ready"
                raise AppError(code, "Configured controller is not powered or ready.", 503)
            return adapter
        raise AppError(
            "adapter_missing", "Actions controller unavailable; check USB or VM/LXC exposure.", 503
        )
    finally:
        if bus:
            bus.disconnect()


class BleakTransport:
    def __init__(self, config: ServerConfig):
        self.config = config
        self.adapter_id = ""
        self.client: BleakClient | None = None
        self.verified = False
        self.subscribed = False
        self.rssi: int | None = None
        self.setup_deadline = 0.0

    async def acquire(self, phase: Callable[[str], None]) -> None:
        if self.client is not None:
            raise AppError("cleanup_failed", "Previous client has not been released.", 503)
        self.adapter_id = await asyncio.wait_for(
            resolve_adapter(self.config.bluetooth.adapter_address), 5
        )
        logging.getLogger("uvicorn.error").info(
            "Bluetooth controller found and ready: %s (%s). Scanning for station %s.",
            self.config.bluetooth.adapter_address,
            self.adapter_id,
            self.config.device.address,
        )
        phase("scanning")
        rssi: list[int] = []

        def match(device: Any, advertisement: Any) -> bool:
            if device.address.upper() == self.config.device.address:
                rssi.append(advertisement.rssi)
                return True
            return False

        device = await BleakScanner.find_device_by_filter(
            match,
            timeout=self.config.bluetooth.scan_timeout_seconds,
            bluez={"adapter": self.adapter_id},
        )
        if device is None:
            raise AppError(
                "station_not_found",
                "Station not found. Check power, Bluetooth, range and other apps.",
                503,
            )
        self.rssi = rssi[-1] if rssi else None
        if (
            device.address.upper() != self.config.device.address
            or device.name != self.config.device.expected_name
        ):
            raise AppError(
                "identity_mismatch", "Discovered station identity differs from configuration.", 403
            )
        phase("connecting")
        self.client = BleakClient(
            device,
            timeout=self.config.bluetooth.setup_timeout_seconds,
            bluez={"adapter": self.adapter_id},
        )
        self.setup_deadline = (
            asyncio.get_running_loop().time() + self.config.bluetooth.setup_timeout_seconds
        )
        async with asyncio.timeout_at(self.setup_deadline):
            await self.client.connect()
            name = (
                (await self.client.read_gatt_char(NAME_UUID))
                .rstrip(b"\0")
                .decode("utf-8", errors="strict")
            )
            service = self.client.services.get_service(SERVICE_UUID)
            status = service.get_characteristic(STATUS_UUID) if service else None
            control = service.get_characteristic(CONTROL_UUID) if service else None
            if (
                name != self.config.device.expected_name
                or not status
                or "notify" not in status.properties
            ):
                raise AppError(
                    "identity_or_gatt_mismatch", "Expected name/status characteristic missing.", 403
                )
            if not control or "write-without-response" not in control.properties:
                raise AppError("gatt_mismatch", "Qualified control characteristic missing.", 403)
            self.verified = (
                device.address.upper() == STATION_ADDRESS
                and name == STATION_NAME
                and self.config.device.profile == PROFILE
            )

    async def subscribe(self, callback: Callable[[bytes], None]) -> None:
        assert self.client is not None
        async with asyncio.timeout_at(self.setup_deadline):
            await self.client.start_notify(
                STATUS_UUID, lambda characteristic, data: callback(bytes(data))
            )
        self.subscribed = True

    def connected(self) -> bool:
        return self.client is not None and self.client.is_connected

    async def write(self, frame: bytes) -> None:
        validate_control(frame)
        if not self.verified or not self.connected() or not self.subscribed or self.client is None:
            raise AppError("unqualified_identity", "Transport is not qualified for writes.", 403)
        control = self.client.services.get_characteristic(CONTROL_UUID)
        if not control or "write-without-response" not in control.properties:
            raise AppError("gatt_mismatch", "Control characteristic changed.", 403)
        await self.client.write_gatt_char(control, frame, response=False)

    async def cleanup(self) -> None:
        client = self.client
        self.verified = False
        if client is None:
            return
        async with asyncio.timeout(5):
            if self.subscribed and client.is_connected:
                try:
                    await asyncio.wait_for(client.stop_notify(STATUS_UUID), 1)
                except Exception:
                    pass
            await client.disconnect()
            if client.is_connected:
                raise AppError("cleanup_failed", "BLE client is still connected.", 503)
        self.client = None
        self.subscribed = False
