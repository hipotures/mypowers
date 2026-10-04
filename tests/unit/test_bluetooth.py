import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from dbus_fast import Message, MessageType, Variant

from mypowers.bluetooth import transport
from mypowers.bluetooth.simulated import SimulatedTransport
from mypowers.bluetooth.supervisor import Supervisor
from mypowers.contracts import AppError
from mypowers.daemon.service import OwnershipLock, Service
from mypowers.protocol import CONTROL_UUID, NAME_UUID, STATUS_UUID


@pytest.mark.parametrize(
    "name,reason",
    [
        ("org.freedesktop.DBus.Error.ServiceUnknown", "bluez_unavailable"),
        ("org.freedesktop.DBus.Error.AccessDenied", "permission_denied"),
        ("org.bluez.Error.NotReady", "adapter_not_ready"),
    ],
)
def test_structured_error_names(name, reason):
    reply = Message(message_type=MessageType.ERROR, error_name=name, reply_serial=1)
    with pytest.raises(AppError) as caught:
        transport.classify_reply(reply)
    assert caught.value.code == reason
    with pytest.raises(AppError):
        transport.classify_reply(None)


@pytest.mark.parametrize(
    "kind,expected",
    [
        ("bus", "system_bus_unavailable"),
        ("permission", "permission_denied"),
        ("missing", "adapter_missing"),
        ("off", "adapter_off"),
        ("unknown", "adapter_not_ready"),
        ("blocked", "adapter_blocked"),
        ("good", "hci9"),
    ],
)
async def test_adapter_resolution_no_fallback(monkeypatch, kind, expected):
    class Bus:
        async def connect(self):
            if kind == "bus":
                raise FileNotFoundError
            if kind == "permission":
                raise PermissionError
            return self

        async def call(self, message):
            assert message.member == "GetManagedObjects"
            props = {
                "Address": Variant("s", "F4:4E:FC:A1:CB:FF"),
                "Powered": Variant("b", kind in {"good", "blocked"}),
            }
            return SimpleNamespace(
                message_type=MessageType.METHOD_RETURN,
                body=[
                    {} if kind == "missing" else {"/org/bluez/hci9": {"org.bluez.Adapter1": props}}
                ],
            )

        def disconnect(self):
            pass

    monkeypatch.setattr(transport, "MessageBus", lambda **_: Bus())
    monkeypatch.setattr(
        transport, "rfkill", lambda _: None if kind == "unknown" else kind == "blocked"
    )
    if kind == "good":
        assert await transport.resolve_adapter("F4:4E:FC:A1:CB:FF") == expected
    else:
        with pytest.raises(AppError) as caught:
            await transport.resolve_adapter("F4:4E:FC:A1:CB:FF")
        assert caught.value.code == expected


class FakeBLEClient:
    def __init__(self, device, **kwargs):
        assert not isinstance(device, str)
        assert kwargs["bluez"]["adapter"] == "hci9"
        self.is_connected = False
        self.writes = []
        self.fail_cleanup = False
        chars = {
            STATUS_UUID: SimpleNamespace(properties=["notify"]),
            CONTROL_UUID: SimpleNamespace(properties=["write-without-response"]),
        }
        self.service = SimpleNamespace(get_characteristic=chars.get)
        self.services = SimpleNamespace(
            get_service=lambda _: self.service, get_characteristic=chars.get
        )

    async def connect(self):
        self.is_connected = True

    async def read_gatt_char(self, uuid):
        assert uuid == NAME_UUID
        return bytearray(b"AP S300 V2.0\0\0")

    async def start_notify(self, uuid, callback):
        assert uuid == STATUS_UUID
        self.callback = callback

    async def write_gatt_char(self, uuid, frame, *, response):
        assert response is False
        self.writes.append(frame)

    async def stop_notify(self, uuid):
        pass

    async def disconnect(self):
        if self.fail_cleanup:
            raise RuntimeError("cleanup failed")
        self.is_connected = False


async def test_bleak_discovered_object_properties_identity_and_cleanup(config, monkeypatch):
    monkeypatch.setattr(transport, "resolve_adapter", AsyncMock(return_value="hci9"))
    device = SimpleNamespace(address=config.device.address, name=config.device.expected_name)
    scanner = AsyncMock(return_value=device)
    monkeypatch.setattr(transport.BleakScanner, "find_device_by_filter", scanner)
    monkeypatch.setattr(transport, "BleakClient", FakeBLEClient)
    port = transport.BleakTransport(config)
    await port.acquire(lambda _: None)
    assert scanner.call_args.kwargs["bluez"] == {"adapter": "hci9"}
    await port.subscribe(lambda _: None)
    await port.write(bytes.fromhex("a56500b10101001869"))
    client = port.client
    client.fail_cleanup = True
    with pytest.raises(RuntimeError):
        await port.cleanup()
    assert port.client is client
    with pytest.raises(AppError):
        await port.acquire(lambda _: None)
    client.fail_cleanup = False
    await port.cleanup()
    assert port.client is None
    with pytest.raises(AppError):
        await port.write(bytes.fromhex("a56500b10101001869"))


@pytest.mark.parametrize(
    "kind", ["absent", "wrong_address", "wrong_name", "wrong_gatt", "other_unit"]
)
async def test_transport_failed_setup(config, monkeypatch, kind):
    monkeypatch.setattr(transport, "resolve_adapter", AsyncMock(return_value="hci9"))
    device = SimpleNamespace(address=config.device.address, name=config.device.expected_name)
    if kind == "wrong_address":
        device.address = "11:22:33:44:55:66"
    if kind == "wrong_name":
        device.name = "Other"
    if kind == "other_unit":
        config.device.address = device.address = "11:22:33:44:55:66"
    monkeypatch.setattr(
        transport.BleakScanner,
        "find_device_by_filter",
        AsyncMock(return_value=None if kind == "absent" else device),
    )

    class Client(FakeBLEClient):
        async def read_gatt_char(self, uuid):
            return (
                bytearray(b"Wrong") if kind == "wrong_gatt" else await super().read_gatt_char(uuid)
            )

    monkeypatch.setattr(transport, "BleakClient", Client)
    port = transport.BleakTransport(config)
    try:
        if kind == "other_unit":
            await port.acquire(lambda _: None)
            await port.subscribe(lambda _: None)
            with pytest.raises(AppError):
                await port.write(bytes.fromhex("a56500b10101001869"))
        else:
            with pytest.raises(AppError):
                await port.acquire(lambda _: None)
    finally:
        await port.cleanup()


async def test_supervisor_pause_resume_retry_and_single_cleanup(config):
    service = Service(config)
    await service.start()
    try:
        for _ in range(20):
            if service.core.snapshot().controls.allowed:
                break
            await asyncio.sleep(0.05)
        assert service.core.snapshot().controls.allowed
        session = service.core.connection.session_id
        for _ in range(5):
            service.supervisor.wakeup.set()
        await asyncio.sleep(0.1)
        assert service.core.connection.session_id == session
        service.core.connection_intent("paused")
        assert not service.core.snapshot().controls.allowed
        await asyncio.sleep(0.1)
        assert service.core.connection.phase == "paused"
        service.core.connection_intent("running")
        await asyncio.sleep(0.4)
        assert service.core.connection.session_id != session
    finally:
        await service.close()


def test_process_lock(config):
    first = OwnershipLock(config.runtime_dir, "same")
    second = OwnershipLock(config.runtime_dir, "same")
    first.acquire()
    try:
        with pytest.raises(RuntimeError):
            second.acquire()
    finally:
        first.close()
    second.acquire()
    second.close()


async def test_cleanup_failure_prevents_new_port(core, config):
    service = core[0]
    supervisor = Supervisor(service, SimulatedTransport, config.bluetooth)

    class Hung:
        async def cleanup(self):
            raise RuntimeError

    port = Hung()
    supervisor.port = port
    assert await supervisor.release() is False
    assert supervisor.port is port and service.connection.phase == "cleanup_failed"


@pytest.mark.parametrize("failure", ["exception", "cancelled", "clean_exit"])
async def test_essential_task_failure_is_visible_not_healthy(config, failure):
    service = Service(config)
    await service.start()
    try:

        async def broken():
            if failure == "exception":
                raise RuntimeError("injected essential task failure")

        task = asyncio.create_task(broken(), name="injected-task")
        if failure == "cancelled":
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        service.task_done(task)
        assert not service.healthy
        assert "injected-task" in service.task_error
        assert not service.core.snapshot().controls.allowed
    finally:
        await service.close()


async def test_silent_subscription_first_deadline_and_recovery(config):
    class Silent(SimulatedTransport):
        async def subscribe(self, callback):
            self.callback = callback

    config.bluetooth.first_sample_timeout_seconds = 0.03
    service = Service(config, Silent)
    await service.start()
    try:
        await asyncio.sleep(0.3)
        assert service.core.connection.reason_code == "no_telemetry"
        assert not service.core.snapshot().controls.allowed
        assert service.core.latest is None
        service.supervisor.wakeup.set()
    finally:
        await service.close()


async def test_infrastructure_restoration_and_retry_coalescing(config):
    attempts = []
    available = False

    class Port(SimulatedTransport):
        async def acquire(self, phase):
            attempts.append(1)
            if not available:
                raise AppError("adapter_missing", "Injected unavailable controller.", 503)
            await super().acquire(phase)

    service = Service(config, Port)
    await service.start()
    try:
        await asyncio.sleep(0.05)
        assert service.core.connection.reason_code == "adapter_missing"
        available = True
        for _ in range(20):
            service.supervisor.wakeup.set()
        await asyncio.sleep(0.4)
        assert len(attempts) == 2
        assert service.core.snapshot().controls.allowed
    finally:
        await service.close()
