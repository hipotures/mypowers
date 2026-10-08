"""Lifespan resources, observed task failures and local ownership lock."""

import asyncio
import fcntl
import hashlib
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

from mypowers.alerts import BatteryAlerts, ConnectionAlerts, Telegram
from mypowers.bluetooth.simulated import SimulatedTransport
from mypowers.bluetooth.supervisor import Supervisor
from mypowers.bluetooth.transport import BleakTransport, Transport
from mypowers.config.server import ServerConfig
from mypowers.core import Core
from mypowers.diagnostics import Diagnostics
from mypowers.storage import HistoryStore


class OwnershipLock:
    def __init__(self, directory: Path, identity: str):
        self.directory, self.identity = directory, identity
        self.fd: int | None = None

    def acquire(self) -> None:
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        digest = hashlib.sha256(self.identity.encode()).hexdigest()[:24]
        fd = os.open(
            self.directory / f"{digest}.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600
        )
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(fd)
            raise RuntimeError(
                "Another daemon owns this adapter/station in the runtime directory."
            ) from None
        self.fd = fd

    def close(self) -> None:
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None


class Service:
    def __init__(self, config: ServerConfig, factory: Callable[[], Transport] | None = None):
        self.config = config
        self.core = Core(
            address=config.device.address,
            name=config.device.expected_name,
            profile=config.device.profile,
            adapter=config.bluetooth.adapter_address,
            environment=config.environment,
            backend=config.backend,
            stale_after=config.bluetooth.stale_after_seconds,
        )
        self.lock = OwnershipLock(
            config.runtime_dir, config.bluetooth.adapter_address + config.device.address
        )
        self.logs = Diagnostics(
            self.core.instance,
            config.log_dir,
            config.logging.level,
            config.logging.max_file_bytes,
            config.logging.backup_count,
            tuple(
                value
                for value in (config.api_token, config.telegram_bot_token, config.telegram_chat_id)
                if value
            ),
        )
        self.core.log, self.core.log_health = self.logs.log, self.logs.health
        self.history = HistoryStore(
            self.core,
            config.data_dir,
            config.history.enabled,
            config.history.interval_seconds,
            config.battery_alert,
            config.connection_alert,
        )
        self.telegram = Telegram(config.telegram_bot_token, config.telegram_chat_id)
        self.alerts = BatteryAlerts(self.core, self.history, self.telegram)
        self.connection_alerts = ConnectionAlerts(self.core, self.history, self.telegram)
        selected: Callable[[], Transport] = factory or (
            SimulatedTransport if config.backend == "simulated" else lambda: BleakTransport(config)
        )
        self.supervisor = Supervisor(self.core, selected, config.bluetooth)
        self.tasks: list[asyncio.Task[None]] = []
        self.healthy = True
        self.closing = False
        self.task_error: str | None = None

    async def start(self) -> None:
        await asyncio.to_thread(self.lock.acquire)
        try:
            self.logs.start()
            self.logs.log(
                "INFO", "startup", "MyPowers daemon started.", backend=self.config.backend
            )
            self.tasks = [
                asyncio.create_task(self.supervisor.run(), name="ble-supervisor"),
                asyncio.create_task(self.history.run(), name="history-writer"),
                asyncio.create_task(self.monitor(), name="health-monitor"),
                asyncio.create_task(self.alerts.run(), name="battery-alerts"),
                asyncio.create_task(self.connection_alerts.run(), name="connection-alerts"),
            ]
            for task in self.tasks:
                task.add_done_callback(self.task_done)
        except BaseException:
            self.lock.close()
            raise

    def task_done(self, task: asyncio.Task[None]) -> None:
        if self.closing:
            return
        error = None if task.cancelled() else task.exception()
        if error or not self.closing:
            self.healthy = False
            self.task_error = task.get_name() + " stopped unexpectedly"
            self.logs.log(
                "ERROR",
                "essential_task_failed",
                self.task_error,
                error_type="CancelledError"
                if task.cancelled()
                else type(error).__name__
                if error
                else None,
            )
            self.core.invalidate()
            self.core.closing = True
            self.core.publish()

    async def monitor(self) -> None:
        while True:
            self.core.tick()
            self.logs.tick()
            await asyncio.sleep(0.25)

    async def close(self) -> None:
        self.closing = True
        self.core.closing = True
        self.history.stopping = True
        self.supervisor.stop()
        await self.core.close()
        for subscriber in self.core.subscribers:
            if not subscriber.full():
                subscriber.put_nowait({"type": "shutdown"})
        try:
            await asyncio.wait_for(asyncio.shield(self.tasks[0]), 6)
        except (TimeoutError, asyncio.CancelledError):
            self.tasks[0].cancel()
        try:
            await asyncio.wait_for(asyncio.shield(self.tasks[1]), 2)
        except (TimeoutError, asyncio.CancelledError):
            self.tasks[1].cancel()
        for task in self.tasks:
            if not task.done():
                task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)
        await self.history.close()
        self.logs.log("INFO", "shutdown", "Daemon stopped; station outputs were not changed.")
        await self.logs.close()
        self.lock.close()

    def diagnostics(self) -> dict[str, Any]:
        return {"healthy": self.healthy, "task_error": self.task_error}
