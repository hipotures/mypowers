"""Server-owned battery rule, durable coalescing and bounded Telegram delivery."""

import asyncio
import json
from collections.abc import Awaitable, Callable
from typing import Literal
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from pydantic import BaseModel, ConfigDict, Field

from mypowers.contracts import AppError
from mypowers.core import Core
from mypowers.storage import HistoryStore


class AlertState(BaseModel):
    model_config = ConfigDict(extra="forbid", allow_inf_nan=False)
    state: Literal["NORMAL", "ALERT"] = "NORMAL"
    notified: Literal["NORMAL", "ALERT"] = "NORMAL"
    last_notification: float | None = Field(default=None, ge=0)
    last_attempt: float | None = Field(default=None, ge=0)


class Telegram:
    def __init__(self, token: str | None, chat_id: str | None):
        self.token, self.chat_id = token, chat_id
        self.lock = asyncio.Lock()

    @property
    def configured(self) -> bool:
        return bool(self.token and self.chat_id)

    def _send(self, text: str) -> None:
        request = Request(
            f"https://api.telegram.org/bot{self.token}/sendMessage",
            data=json.dumps({"chat_id": self.chat_id, "text": text}).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urlopen(request, timeout=10) as response:  # noqa: S310
                value = json.loads(response.read(65537))
            if not isinstance(value, dict) or value.get("ok") is not True:
                raise AppError("telegram_failed", "Telegram did not confirm delivery.", 502)
        except HTTPError as error:
            raise AppError(
                "telegram_failed", f"Telegram rejected delivery (HTTP {error.code}).", 502
            ) from None
        except (URLError, OSError, ValueError):
            raise AppError("telegram_failed", "Cannot confirm Telegram delivery.", 502) from None

    async def send(self, text: str) -> None:
        if not self.configured:
            raise AppError("telegram_not_configured", "Telegram is not configured on server.", 503)
        async with self.lock:
            try:
                async with asyncio.timeout(12):
                    await asyncio.to_thread(self._send, text)
            except TimeoutError:
                raise AppError("telegram_failed", "Telegram delivery timed out.", 502) from None


class BatteryAlerts:
    def __init__(
        self,
        core: Core,
        store: HistoryStore,
        telegram: Telegram,
        send: Callable[[str], Awaitable[None]] | None = None,
    ):
        self.core, self.store, self.telegram = core, store, telegram
        self.send = send or telegram.send
        self.state = AlertState()
        self.loaded = False
        self.reload = True
        self.rule = store.battery_alert_defaults
        self.delivery: asyncio.Task[None] | None = None
        self.state_lock = asyncio.Lock()
        self.dirty = False

    async def save(self) -> None:
        self.dirty = True
        await self.store.alert_state(self.state.model_dump())
        self.dirty = False

    async def evaluate(self, battery: int) -> None:
        previous = self.state.state
        if previous == "NORMAL" and battery <= self.rule.threshold_percent:
            self.state.state = "ALERT"
        elif previous == "ALERT" and battery >= (
            self.rule.threshold_percent + self.rule.hysteresis_percent
        ):
            self.state.state = "NORMAL"
        if previous != self.state.state:
            await self.save()
            self.core.log(
                "WARNING" if self.state.state == "ALERT" else "INFO",
                "battery_alert" if self.state.state == "ALERT" else "battery_recovered",
                f"Battery {'low' if self.state.state == 'ALERT' else 'recovered'}: {battery}%",
                battery_percent=battery,
            )

    async def step(self) -> None:
        if self.delivery is not None and self.delivery.done():
            delivery, self.delivery = self.delivery, None
            delivery.result()
        if self.store.db is None:
            return
        async with self.state_lock:
            if not self.loaded:
                saved = await self.store.alert_state()
                self.state = AlertState.model_validate(saved or {})
                self.loaded = True
            if self.dirty:
                await self.save()
            if self.reload:
                self.rule = (await self.store.settings()).battery_alert
                self.reload = False
            if not self.rule.enabled or self.core.telemetry_state() != "live":
                return
            assert self.core.latest is not None
            battery = self.core.latest.sample.battery_percent
            await self.evaluate(battery)
            if self.state.state == self.state.notified or not self.telegram.configured:
                return
            if self.delivery is not None and not self.delivery.done():
                return
            now = self.core.clock.time()
            cooldown = self.rule.min_notification_interval_minutes * 60
            if (
                self.state.last_notification is not None
                and now < self.state.last_notification + cooldown
            ):
                return
            # Reserve before network I/O so restart/uncertain delivery cannot retry immediately.
            if (
                self.state.last_attempt is not None
                and (
                    self.state.last_notification is None
                    or self.state.last_attempt > self.state.last_notification
                )
                and now < self.state.last_attempt + max(30, cooldown)
            ):
                return
            previous_attempt = self.state.last_attempt
            self.state.last_attempt = now
            await self.save()
            target = self.state.state
            self.delivery = asyncio.create_task(self.deliver(target, previous_attempt))

    async def deliver(
        self, target: Literal["NORMAL", "ALERT"], previous_attempt: float | None
    ) -> None:
        async with self.state_lock:
            if self.reload:
                self.rule = (await self.store.settings()).battery_alert
                self.reload = False
            fresh = self.rule.enabled and self.core.telemetry_state() == "live"
            if fresh:
                assert self.core.latest is not None
                battery = self.core.latest.sample.battery_percent
                await self.evaluate(battery)
            if not fresh or self.state.state != target or self.state.notified == target:
                self.state.last_attempt = previous_attempt
                await self.save()
                return
            text = f"Battery {'low' if target == 'ALERT' else 'recovered'}: {battery}%"
        try:
            await self.send(text)
        except Exception:
            self.core.log(
                "ERROR", "notification_failed", "Telegram delivery failed; latest state retained."
            )
            return
        async with self.state_lock:
            self.state.notified = target
            self.state.last_notification = self.core.clock.time()
            await self.save()
        self.core.log("INFO", "notification_sent", text, connector="telegram")

    async def run(self) -> None:
        try:
            while True:
                try:
                    await self.step()
                except AppError:
                    self.reload = True
                await asyncio.sleep(0.25)
        finally:
            if self.delivery is not None:
                self.delivery.cancel()
                await asyncio.gather(self.delivery, return_exceptions=True)

    async def test(self) -> None:
        try:
            await self.telegram.send("MyPowers: Telegram test notification.")
        except AppError:
            self.core.log("ERROR", "notification_test_failed", "Telegram test notification failed.")
            raise
        self.core.log("INFO", "notification_test", "Telegram test notification sent.")


class ConnectionAlertState(AlertState):
    unavailable_since: float | None = Field(default=None, ge=0)
    last_sample_at: str | None = None
    reason_code: str | None = None


class ConnectionAlerts(BatteryAlerts):
    """Durable outage/recovery episodes; fresh data must survive recovery hysteresis."""

    state: ConnectionAlertState

    def __init__(
        self,
        core: Core,
        store: HistoryStore,
        telegram: Telegram,
        send: Callable[[str], Awaitable[None]] | None = None,
    ):
        super().__init__(core, store, telegram, send)
        self.state = ConnectionAlertState()
        self.connection_rule = store.connection_alert_defaults
        # A process restart must observe recovery again, not trust an old timer.
        self.live_since: float | None = None
        self.live_segment: str | None = None

    async def save(self) -> None:
        self.dirty = True
        await self.store.connection_alert_state(self.state.model_dump())
        self.dirty = False

    async def evaluate_connection(self) -> bool:
        if self.reload:
            self.connection_rule = (await self.store.settings()).connection_alert
            self.reload = False
        before = self.state.model_dump()
        if not self.connection_rule.enabled:
            self.state = ConnectionAlertState(
                last_notification=self.state.last_notification, last_attempt=self.state.last_attempt
            )
            self.live_since = None
            active = False
        elif self.core.connection.desired == "paused" or self.core.closing:
            if self.state.notified == "NORMAL":
                self.state.state = "NORMAL"
            self.state.unavailable_since = None
            self.live_since = None
            active = False
        elif self.core.telemetry_state() == "live":
            assert self.core.latest is not None
            segment = self.core.latest.sample.segment_id
            if self.live_since is None or self.live_segment != segment:
                self.live_since = self.core.clock.monotonic()
            self.live_segment = segment
            # An outage shorter than the alarm threshold is not an episode.
            if self.state.state == "NORMAL" and self.state.notified == "NORMAL":
                self.state.unavailable_since = None
            elif (
                self.core.clock.monotonic() - self.live_since
                >= self.connection_rule.recovery_seconds
            ):
                self.state.state = "NORMAL"
                self.state.unavailable_since = None
            active = True
        else:
            self.live_since = None
            now = self.core.clock.time()
            reason = self.core.connection.reason_code
            if reason:
                self.state.reason_code = reason
            if self.core.latest:
                self.state.last_sample_at = self.core.latest.sample.received_at
            if self.state.unavailable_since is None:
                self.state.unavailable_since = now
            if now - self.state.unavailable_since >= self.connection_rule.outage_seconds:
                self.state.state = "ALERT"
            active = True
        if before != self.state.model_dump():
            await self.save()
            if before["state"] != self.state.state:
                self.core.log(
                    "WARNING" if self.state.state == "ALERT" else "INFO",
                    "connection_alert" if self.state.state == "ALERT" else "connection_recovered",
                    "Bluetooth data unavailable."
                    if self.state.state == "ALERT"
                    else "Fresh Bluetooth data recovered.",
                )
        return active

    async def step(self) -> None:
        if self.delivery is not None and self.delivery.done():
            delivery, self.delivery = self.delivery, None
            delivery.result()
        if self.store.db is None:
            return
        async with self.state_lock:
            if not self.loaded:
                saved = await self.store.connection_alert_state()
                self.state = ConnectionAlertState.model_validate(saved or {})
                self.loaded = True
            if self.dirty:
                await self.save()
            if not await self.evaluate_connection():
                return
            # Recovery is eligible only after uninterrupted live telemetry.
            if (
                self.state.state == "NORMAL"
                and self.state.notified == "ALERT"
                and (
                    self.live_since is None
                    or self.core.clock.monotonic() - self.live_since
                    < self.connection_rule.recovery_seconds
                )
            ):
                return
            if self.state.state == self.state.notified or not self.telegram.configured:
                return
            if self.delivery is not None and not self.delivery.done():
                return
            now = self.core.clock.time()
            cooldown = self.connection_rule.min_notification_interval_minutes * 60
            if (
                self.state.last_notification is not None
                and now < self.state.last_notification + cooldown
            ):
                return
            if (
                self.state.last_attempt is not None
                and (
                    self.state.last_notification is None
                    or self.state.last_attempt > self.state.last_notification
                )
                and now < self.state.last_attempt + max(30, cooldown)
            ):
                return
            previous_attempt = self.state.last_attempt
            self.state.last_attempt = now
            await self.save()
            self.delivery = asyncio.create_task(self.deliver(self.state.state, previous_attempt))

    def outage_reason(self) -> str:
        phase = (
            self.core.connection.reason_code or self.state.reason_code or self.core.connection.phase
        )
        reasons = {
            "adapter_missing": "Bluetooth controller missing.",
            "adapter_off": "Bluetooth controller powered off.",
            "adapter_blocked": "Bluetooth controller blocked.",
            "adapter_not_ready": "Bluetooth controller not ready.",
            "bluez_unavailable": "Bluetooth service unavailable.",
            "system_bus_unavailable": "Bluetooth system bus unavailable.",
            "permission_denied": "Bluetooth access denied.",
            "station_not_found": (
                "S300 not detected (station Bluetooth/power, range or another client)."
            ),
            "connection_failed": "S300 Bluetooth connection setup failed.",
            "connection_lost": "S300 Bluetooth link lost.",
        }
        return reasons.get(phase, "Fresh S300 telemetry unavailable.")

    async def deliver(
        self, target: Literal["NORMAL", "ALERT"], previous_attempt: float | None
    ) -> None:
        async with self.state_lock:
            active = await self.evaluate_connection()
            eligible = active and self.state.state == target and self.state.notified != target
            if target == "ALERT":
                eligible = eligible and self.core.telemetry_state() != "live"
            else:
                eligible = (
                    eligible
                    and self.live_since is not None
                    and (
                        self.core.clock.monotonic() - self.live_since
                        >= self.connection_rule.recovery_seconds
                    )
                )
            if not eligible:
                self.state.last_attempt = previous_attempt
                await self.save()
                return
            if target == "ALERT":
                last = self.state.last_sample_at or "no sample received"
                text = (
                    f"MyPowers: Bluetooth connection alert. {self.outage_reason()} "
                    f"Last data: {last}."
                )
            else:
                text = "MyPowers: Bluetooth connection recovered; fresh S300 telemetry is stable."
        try:
            await self.send(text)
        except Exception:
            self.core.log(
                "ERROR",
                "connection_notification_failed",
                "Telegram connection alert delivery failed; latest state retained.",
            )
            return
        async with self.state_lock:
            self.state.notified = target
            self.state.last_notification = self.core.clock.time()
            await self.save()
        self.core.log("INFO", "connection_notification_sent", text, connector="telegram")
