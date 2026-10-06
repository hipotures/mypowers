import asyncio
import json
from unittest.mock import AsyncMock
from urllib.error import HTTPError, URLError

import pytest
from conftest import frame
from pydantic import ValidationError

from mypowers.alerts import BatteryAlerts, Telegram
from mypowers.contracts import AppError, BatteryAlert, SettingsUpdate
from mypowers.storage import HistoryStore


async def setup_rule(core, tmp_path, **values):
    service, clock, _, session = core
    store = HistoryStore(service, tmp_path, False, 10, BatteryAlert(**values))
    await store.open()
    sender = AsyncMock()
    alerts = BatteryAlerts(service, store, Telegram("test-token", "test-chat"), sender)
    return alerts, store, sender


async def observe(alerts, core, battery, seconds=0):
    service, clock, _, session = core
    clock.advance(seconds)
    service.receive(frame(battery=battery), session)
    await alerts.step()
    if alerts.delivery is not None:
        await alerts.delivery


async def test_hysteresis_and_single_alert_recovery(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    try:
        for battery in [21, 20, 18, 19, 21, 23]:
            await observe(alerts, core, battery)
        assert [c.args[0] for c in sender.call_args_list] == ["Battery low: 20%"]
        await observe(alerts, core, 25, 600)
        for battery in [26, 30, 99]:
            await observe(alerts, core, battery)
        assert [c.args[0] for c in sender.call_args_list] == [
            "Battery low: 20%",
            "Battery recovered: 25%",
        ]
    finally:
        await store.close()


async def test_cooldown_collapses_oscillations_without_repeated_alert(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    try:
        await observe(alerts, core, 20)
        await observe(alerts, core, 25, 240)
        await observe(alerts, core, 19, 120)
        await observe(alerts, core, 18, 240)
        assert sender.call_count == 1
        await observe(alerts, core, 26)
        assert sender.call_args.args == ("Battery recovered: 26%",)
        assert sender.call_count == 2
    finally:
        await store.close()


async def test_pending_recovery_uses_current_battery_at_cooldown_expiry(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    try:
        await observe(alerts, core, 20)
        await observe(alerts, core, 25, 240)
        await observe(alerts, core, 26, 360)
        assert sender.call_args.args == ("Battery recovered: 26%",)
    finally:
        await store.close()


async def test_no_evaluation_or_delivery_from_stale_disconnected_invalid_or_paused(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    service, clock, _, session = core
    try:
        service.receive(frame(battery=20), session)
        clock.advance(4)
        await alerts.step()
        assert alerts.state.state == "NORMAL"
        await observe(alerts, core, 20)
        await observe(alerts, core, 25, 10)
        clock.advance(600)
        await alerts.step()
        assert sender.call_count == 1
        service.set_phase("paused", link_connected=False)
        await alerts.step()
        assert sender.call_count == 1
        service.invalid = True
        await alerts.step()
        assert sender.call_count == 1
    finally:
        await store.close()


async def test_restart_keeps_pending_recovery_and_last_notification(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    try:
        await observe(alerts, core, 20)
        await observe(alerts, core, 25, 240)
        saved = await store.alert_state()
    finally:
        await store.close()
    reopened = HistoryStore(core[0], tmp_path, False, 10)
    await reopened.open()
    restarted = BatteryAlerts(core[0], reopened, Telegram("test", "chat"), sender)
    try:
        await restarted.step()
        assert restarted.state.model_dump() == saved
        assert sender.call_count == 1
        await observe(restarted, core, 26, 360)
        assert sender.call_count == 2
        assert sender.call_args.args == ("Battery recovered: 26%",)
        # Another restart after delivery must not repeat recovered.
        another = BatteryAlerts(core[0], reopened, Telegram("test", "chat"), sender)
        await another.step()
        assert sender.call_count == 2
    finally:
        await reopened.close()


async def test_failed_delivery_keeps_only_latest_state_and_persists_attempt(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    sender.side_effect = RuntimeError("secret-bearing error must never be logged")
    try:
        await observe(alerts, core, 20)
        assert alerts.state.notified == "NORMAL"
        restarted = BatteryAlerts(core[0], store, Telegram("test", "chat"), sender)
        await restarted.step()
        assert sender.call_count == 1
        await observe(restarted, core, 26, 600)
        assert sender.call_count == 1  # No low/recovered replay after failed low delivery.
    finally:
        await store.close()


async def test_slow_delivery_does_not_block_latest_state_evaluation(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path, min_notification_interval_minutes=0)
    entered, release = asyncio.Event(), asyncio.Event()

    async def blocked(text):
        entered.set()
        await release.wait()

    sender.side_effect = blocked
    try:
        core[0].receive(frame(battery=20), core[3])
        await alerts.step()
        await entered.wait()
        core[0].receive(frame(battery=26), core[3])
        await alerts.step()
        assert alerts.state.state == "NORMAL"
        release.set()
        await alerts.delivery
        await observe(alerts, core, 26)
        assert [c.args[0] for c in sender.call_args_list] == [
            "Battery low: 20%",
            "Battery recovered: 26%",
        ]
    finally:
        release.set()
        await store.close()


@pytest.mark.parametrize("change", ["stale", "recovered", "disabled"])
async def test_delivery_rechecks_live_current_state_before_sending(core, tmp_path, change):
    alerts, store, sender = await setup_rule(core, tmp_path)
    try:
        core[0].receive(frame(battery=20), core[3])
        await alerts.step()
        if change == "stale":
            core[1].advance(4)
        elif change == "recovered":
            core[0].receive(frame(battery=26), core[3])
        else:
            alerts.rule = alerts.rule.model_copy(update={"enabled": False})
        await alerts.delivery
        sender.assert_not_awaited()
        assert alerts.state.last_attempt is None
    finally:
        await store.close()


async def test_settings_apply_without_tui_and_disabled_rule_never_sends(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path, enabled=False)
    try:
        await observe(alerts, core, 1)
        assert sender.call_count == 0
        await store.settings(
            SettingsUpdate(battery_alert=BatteryAlert(enabled=True, threshold_percent=40))
        )
        alerts.reload = True
        await observe(alerts, core, 35)
        assert sender.call_args.args == ("Battery low: 35%",)
    finally:
        await store.close()


async def test_test_message_does_not_change_alert_state_or_cooldown(core, tmp_path):
    alerts, store, sender = await setup_rule(core, tmp_path)
    alerts.telegram.send = AsyncMock()
    try:
        await observe(alerts, core, 20)
        saved = await store.alert_state()
        await alerts.test()
        alerts.telegram.send.assert_awaited_once_with("MyPowers: Telegram test notification.")
        assert await store.alert_state() == saved
    finally:
        await store.close()


async def test_confirmed_delivery_retries_failed_state_save_without_resending(
    core, tmp_path, monkeypatch
):
    alerts, store, sender = await setup_rule(core, tmp_path)
    original = store.alert_state
    failed = False

    async def fail_confirmation_once(value=None):
        nonlocal failed
        if value and value["notified"] == "ALERT" and not failed:
            failed = True
            raise AppError("settings_unavailable", "Storage temporarily unavailable.", 503)
        return await original(value)

    monkeypatch.setattr(store, "alert_state", fail_confirmation_once)
    try:
        core[0].receive(frame(battery=20), core[3])
        await alerts.step()
        with pytest.raises(AppError):
            await alerts.delivery
        with pytest.raises(AppError):
            await alerts.step()
        await alerts.step()
        assert (await store.alert_state())["notified"] == "ALERT"
        assert sender.call_count == 1
    finally:
        await store.close()


async def test_partial_rule_update_keeps_other_fields_and_rejects_unreachable_recovery(
    core, tmp_path
):
    alerts, store, sender = await setup_rule(
        core, tmp_path, threshold_percent=10, hysteresis_percent=80
    )
    try:
        result = await store.settings(
            SettingsUpdate(battery_alert=BatteryAlert(min_notification_interval_minutes=3))
        )
        assert result.battery_alert.threshold_percent == 10
        assert result.battery_alert.hysteresis_percent == 80
        with pytest.raises(AppError, match="Recovery threshold"):
            await store.settings(SettingsUpdate(battery_alert=BatteryAlert(threshold_percent=30)))
        assert (await store.settings()).battery_alert == result.battery_alert
    finally:
        await store.close()


@pytest.mark.parametrize(
    "values",
    [
        {"threshold_percent": 99, "hysteresis_percent": 5},
        {"threshold_percent": True},
        {"hysteresis_percent": 0},
        {"min_notification_interval_minutes": -1},
    ],
)
def test_invalid_rule(values):
    with pytest.raises(ValidationError):
        BatteryAlert(**values)


async def test_telegram_missing_configuration():
    with pytest.raises(AppError, match="not configured"):
        await Telegram(None, None).send("test")


@pytest.mark.parametrize("response", [b'{"ok":true}', b'{"ok":false}', b"invalid"])
async def test_telegram_request_and_validation(monkeypatch, response):
    from io import BytesIO

    def open_request(request, timeout):
        assert request.full_url == "https://api.telegram.org/botfake-token/sendMessage"
        assert json.loads(request.data) == {"chat_id": "123", "text": "test"}
        assert timeout == 10
        return BytesIO(response)

    monkeypatch.setattr("mypowers.alerts.urlopen", open_request)
    telegram = Telegram("fake-token", "123")
    if response == b'{"ok":true}':
        await telegram.send("test")
    else:
        with pytest.raises(AppError):
            await telegram.send("test")


@pytest.mark.parametrize(
    "error", [HTTPError("secret-url", 403, "secret-token", None, None), URLError("secret-token")]
)
async def test_telegram_errors_do_not_expose_secrets(monkeypatch, error):
    def fail(*args, **kwargs):
        raise error

    monkeypatch.setattr("mypowers.alerts.urlopen", fail)
    with pytest.raises(AppError) as caught:
        await Telegram("fake-token", "123").send("test")
    assert "secret" not in caught.value.message
