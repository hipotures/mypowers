"""One serial acquisition/cleanup owner, with coalesced wakeups."""

import asyncio
import random
from collections.abc import Callable
from functools import partial

from mypowers.bluetooth.transport import Transport
from mypowers.config.server import Bluetooth
from mypowers.contracts import AppError
from mypowers.core import Core

INFRASTRUCTURE = frozenset(
    {
        "adapter_missing",
        "adapter_off",
        "adapter_blocked",
        "adapter_not_ready",
        "system_bus_unavailable",
        "bluez_unavailable",
        "permission_denied",
    }
)


class Supervisor:
    def __init__(
        self,
        core: Core,
        factory: Callable[[], Transport],
        timing: Bluetooth,
        jitter: Callable[[], float] = random.random,
    ):
        self.core, self.factory, self.timing, self.jitter = core, factory, timing, jitter
        self.wakeup = asyncio.Event()
        self.stopping = False
        self.port: Transport | None = None
        self.core.wake = self.wakeup.set

    def paused(self) -> bool:
        return self.core.connection.desired == "paused"

    async def delay(self, seconds: float) -> None:
        try:
            await asyncio.wait_for(self.wakeup.wait(), seconds)
        except TimeoutError:
            pass
        self.wakeup.clear()

    async def release(self) -> bool:
        if self.port is None:
            return True
        self.core.end_session()
        try:
            await asyncio.wait_for(self.port.cleanup(), 5.5)
        except Exception:
            self.core.set_phase(
                "cleanup_failed",
                "cleanup_failed",
                "Old BLE session could not be released.",
                link_connected=False,
            )
            return False
        self.port = None
        return True

    async def run(self) -> None:
        failures = 0
        try:
            while not self.stopping:
                if self.port and not await self.release():
                    await self.delay(10)
                    continue
                if self.paused():
                    self.core.set_phase("paused", link_connected=False)
                    await self.delay(30)
                    continue
                self.core.reconnect_attempts += 1
                self.core.set_phase("reconnecting", link_connected=False)
                self.port = self.factory()
                reason = "connection_failed"
                try:
                    await self.port.acquire(lambda phase: self.core.set_phase(phase))
                    if self.stopping or self.paused():
                        continue
                    session = self.core.begin_session(self.port, self.port.adapter_id)
                    self.core.rssi = getattr(self.port, "rssi", None)
                    self.core.rssi_monotonic = (
                        self.core.clock.monotonic() if self.core.rssi is not None else None
                    )
                    self.core.rssi_at = (
                        self.core.clock.time() if self.core.rssi is not None else None
                    )
                    # The transport's setup deadline includes subscription.
                    await self.port.subscribe(partial(self.core.receive, session=session))
                    started = self.core.clock.monotonic()
                    while not self.stopping and self.core.connection.desired == "running":
                        self.core.tick()
                        if not self.port.connected():
                            raise AppError("connection_lost", "BLE link lost.", 503)
                        if self.core.resync_required:
                            break
                        latest = self.core.latest
                        if not latest or latest.session != session:
                            if (
                                self.core.clock.monotonic() - started
                                >= self.timing.first_sample_timeout_seconds
                            ):
                                raise AppError(
                                    "no_telemetry", "Connected, but no valid status arrived.", 503
                                )
                        else:
                            failures = 0
                            if (self.core.age() or 0) >= self.timing.reconnect_after_seconds:
                                raise AppError(
                                    "telemetry_silence", "Station data stopped updating.", 503
                                )
                        await self.delay(0.25)
                except asyncio.CancelledError:
                    raise
                except Exception as error:
                    reason = error.code if isinstance(error, AppError) else "connection_failed"
                    self.core.set_phase(
                        reason,
                        reason,
                        error.message
                        if isinstance(error, AppError)
                        else "BLE setup failed; inspect application diagnostics.",
                        link_connected=False,
                    )
                    self.core.log(
                        "DEBUG",
                        "connection_exception",
                        "Transport failure.",
                        error_type=type(error).__name__,
                        dbus_error=getattr(error, "dbus_error", None),
                    )
                await self.release()
                if self.stopping:
                    break
                self.core.resync_required = False
                if self.paused():
                    continue
                seconds = (
                    10.0
                    if reason in INFRASTRUCTURE
                    else (2.0, 5.0, 10.0, 20.0, 30.0)[min(failures, 4)]
                )
                failures += 1
                seconds *= 1 + 0.1 * self.jitter()
                self.core.set_phase(reason, reason, retry_in_seconds=seconds, link_connected=False)
                await self.delay(seconds)
        finally:
            await self.release()

    def stop(self) -> None:
        self.stopping = True
        self.wakeup.set()
