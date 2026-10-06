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
