import asyncio
from unittest.mock import AsyncMock

import pytest
from conftest import frame
from pydantic import ValidationError

from mypowers.alerts import ConnectionAlerts, Telegram
from mypowers.contracts import ConnectionAlert, SettingsUpdate
from mypowers.storage import HistoryStore


async def setup(core, tmp_path, **values):
    store = HistoryStore(
        core[0],
        tmp_path,
        False,
        10,
        connection_alert=ConnectionAlert(min_notification_interval_minutes=0, **values),
    )
    await store.open()
    send = AsyncMock()
    alerts = ConnectionAlerts(core[0], store, Telegram("fake", "fake"), send)
    await alerts.step()
    return alerts, store, send


async def step(alerts, core, seconds=0, live=False):
    if live and seconds > 0:
        # Simulate uninterrupted samples, not just one frame after a silent gap.
        for _ in range(int(seconds)):
            core[1].advance(1)
            core[0].receive(frame(), core[3])
            await alerts.step()
            if alerts.delivery is not None:
                await alerts.delivery
        return
    core[1].advance(seconds)
    if live:
        core[0].receive(frame(), core[3])
    await alerts.step()
    if alerts.delivery is not None:
        await alerts.delivery


async def test_outage_delay_single_alarm_and_stable_recovery(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path)
    try:
        await step(alerts, core, 4)
        await step(alerts, core, 59)
        send.assert_not_awaited()
        await step(alerts, core, 1)
        assert send.call_count == 1
        assert "Last data:" in send.call_args.args[0]
        await step(alerts, core, 500)
        assert send.call_count == 1
        await step(alerts, core, live=True)
        await step(alerts, core, 10, live=True)
        assert send.call_count == 1
        await step(alerts, core, 4)  # Staleness resets recovery hysteresis.
        await step(alerts, core, live=True)
        await step(alerts, core, 15, live=True)
        assert send.call_count == 2
        assert "recovered" in send.call_args.args[0]
        await step(alerts, core, live=True)
        assert send.call_count == 2
    finally:
        await store.close()


async def test_short_gap_pause_and_disabled_rule_are_silent(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path)
    try:
        await step(alerts, core, 4)
        await step(alerts, core, 59, live=True)
        await step(alerts, core, 4)
        core[0].connection_intent("paused")
        await step(alerts, core, 1000)
        core[0].connection_intent("running")
        await step(alerts, core)
        await step(alerts, core, 59)
        send.assert_not_awaited()
        await store.settings(SettingsUpdate(connection_alert=ConnectionAlert(enabled=False)))
        alerts.reload = True
        await step(alerts, core, 1000)
        send.assert_not_awaited()
    finally:
        await store.close()


async def test_restart_preserves_outage_and_does_not_repeat_sent_alert(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path)
    try:
        await step(alerts, core, 4)
        await step(alerts, core, 30)
        restarted = ConnectionAlerts(core[0], store, alerts.telegram, send)
        await step(restarted, core, 30)
        assert send.call_count == 1
        again = ConnectionAlerts(core[0], store, alerts.telegram, send)
        await step(again, core, 600)
        assert send.call_count == 1
        await step(again, core, live=True)
        await step(again, core, 15, live=True)
        assert send.call_count == 2
    finally:
        await store.close()


async def test_cooldown_coalesces_failed_delivery_and_rapid_flapping(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path, outage_seconds=1, recovery_seconds=1)
    try:
        await store.settings(
            SettingsUpdate(
                connection_alert=ConnectionAlert(
                    outage_seconds=1, recovery_seconds=1, min_notification_interval_minutes=2
                )
            )
        )
        alerts.reload = True
        send.side_effect = RuntimeError("private-token")
        await step(alerts, core, 4)
        await step(alerts, core, 1)
        assert send.call_count == 1
        restarted = ConnectionAlerts(core[0], store, alerts.telegram, send)
        await step(restarted, core, 119)
        assert send.call_count == 1
        send.side_effect = None
        await step(restarted, core, 1)
        assert send.call_count == 2
        await step(restarted, core, live=True)
        await step(restarted, core, 1, live=True)
        assert send.call_count == 2
        await step(restarted, core, 4)
        await step(restarted, core, 120)
        assert send.call_count == 2  # Already notified about the continuing episode.
        await step(restarted, core, live=True)
        await step(restarted, core, 1, live=True)
        assert send.call_count == 3
    finally:
        await store.close()


async def test_adapter_reason_and_settings_partial_update_persist(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path)
    try:
        await store.settings(SettingsUpdate(connection_alert=ConnectionAlert(outage_seconds=7)))
        settings = await store.settings(
            SettingsUpdate(connection_alert=ConnectionAlert(enabled=False))
        )
        assert settings.connection_alert.outage_seconds == 7
        settings = await store.settings(
            SettingsUpdate(connection_alert=ConnectionAlert(enabled=True))
        )
        alerts.reload = True
        core[0].set_phase("adapter_missing", "adapter_missing", link_connected=False)
        await step(alerts, core)
        await step(alerts, core, 7)
        assert "controller missing" in send.call_args.args[0]
        assert settings.battery_alert.enabled
    finally:
        await store.close()


async def test_delivery_rechecks_pause_and_does_not_block_monitoring(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path, outage_seconds=1)
    try:
        await step(alerts, core, 4)
        core[1].advance(1)
        await alerts.step()
        core[0].connection_intent("paused")
        await alerts.delivery
        send.assert_not_awaited()
        core[0].connection_intent("running")
        await step(alerts, core)
        entered, release = asyncio.Event(), asyncio.Event()

        async def blocked(text):
            entered.set()
            await release.wait()

        send.side_effect = blocked
        core[1].advance(1)
        await alerts.step()
        await entered.wait()
        await alerts.step()  # Network I/O must not hold the alert-state lock.
        release.set()
        await alerts.delivery
        assert send.call_count == 1
    finally:
        await store.close()


@pytest.mark.parametrize(
    "values",
    [
        {"enabled": 1},
        {"outage_seconds": 0},
        {"recovery_seconds": True},
        {"min_notification_interval_minutes": -1},
    ],
)
def test_invalid_connection_rule(values):
    with pytest.raises(ValidationError):
        ConnectionAlert(**values)


async def test_confirmed_connection_delivery_save_failure_does_not_resend(
    core, tmp_path, monkeypatch
):
    from mypowers.contracts import AppError

    alerts, store, send = await setup(core, tmp_path, outage_seconds=1)
    original = store.connection_alert_state
    failed = False

    async def fail_once(value=None):
        nonlocal failed
        if value and value["notified"] == "ALERT" and not failed:
            failed = True
            raise AppError("settings_unavailable", "Storage unavailable.", 503)
        return await original(value)

    monkeypatch.setattr(store, "connection_alert_state", fail_once)
    try:
        await step(alerts, core, 4)
        core[1].advance(1)
        await alerts.step()
        with pytest.raises(AppError):
            await alerts.delivery
        with pytest.raises(AppError):
            await alerts.step()
        await alerts.step()
        assert (await store.connection_alert_state())["notified"] == "ALERT"
        assert send.call_count == 1
    finally:
        await store.close()


async def test_new_table_preserves_existing_battery_state_and_saved_rule(core, tmp_path):
    # Simulate the previous release's database without the new table.
    alerts, store, send = await setup(core, tmp_path)
    battery_state = {
        "state": "ALERT",
        "notified": "ALERT",
        "last_notification": 123.0,
        "last_attempt": 122.0,
    }
    await store.alert_state(battery_state)
    await store.settings(
        SettingsUpdate(
            connection_alert=ConnectionAlert(enabled=False, outage_seconds=90, recovery_seconds=20)
        )
    )
    await store.db.execute("DROP TABLE connection_alert_state")
    await store.db.commit()
    await store.close()
    reopened = HistoryStore(core[0], tmp_path, False, 10)
    await reopened.open()
    try:
        assert await reopened.alert_state() == battery_state
        assert await reopened.connection_alert_state() is None
        rule = (await reopened.settings()).connection_alert
        assert not rule.enabled and rule.outage_seconds == 90 and rule.recovery_seconds == 20
    finally:
        await reopened.close()


async def test_recovery_detects_silent_gap_even_when_monitor_did_not_tick(core, tmp_path):
    alerts, store, send = await setup(core, tmp_path, outage_seconds=1, recovery_seconds=5)
    try:
        await step(alerts, core, 4)
        await step(alerts, core, 1)
        await step(alerts, core, live=True)
        core[1].advance(10)
        core[0].receive(frame(), core[3])
        await alerts.step()
        assert send.call_count == 1  # The changed telemetry segment resets recovery.
        await step(alerts, core, 5, live=True)
        assert send.call_count == 2
    finally:
        await store.close()
