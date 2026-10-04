"""Offline safety regression tests for ``control_allpowers``.

The tests use only the standard library and never open a Bluetooth connection.  The
captured status frames and control frames below are the evidence boundary for the
verified S300 profile.
"""

from __future__ import annotations

import asyncio
import json
import unittest
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from typing import Self
from unittest.mock import AsyncMock, patch

import control_allpowers as control

BEFORE_AC_OFF = bytes.fromhex("a565b1000108010e61000000000c3923")
AFTER_AC_OFF = bytes.fromhex("a565b1000108010c61000000000c3921")
DC_ON = bytes.fromhex("a565b1000108010f610000001301e5e0")
LIGHT_ON = bytes.fromhex("a565b1000108011e600000001301afba")

DEVICE = type("Device", (), {"address": control.STATION_ADDRESS, "name": control.STATION_NAME})()


def make_status(flags: int, received_at: float) -> control.Status:
    return control.Status(
        flags=flags,
        battery_percent=97,
        input_power_w=0,
        output_power_w=0,
        remaining_minutes=0,
        received_at=received_at,
    )


def recompute_checksum(frame: bytes | bytearray) -> bytes:
    body = bytes(frame[:-1])
    checksum = 0
    for value in body:
        checksum ^= value
    return body + bytes((checksum,))


class FakeClient:
    def __init__(self, *, connected: bool = True) -> None:
        self.is_connected = connected
        self.writes: list[tuple[str, bytes, bool]] = []

    async def write_gatt_char(self, uuid: str, payload: bytes, response: bool = False) -> None:
        self.writes.append((uuid, bytes(payload), response))


class FakeCharacteristic:
    def __init__(self, *properties: str) -> None:
        self.properties = set(properties)


class FakeService:
    def __init__(self) -> None:
        self.status = FakeCharacteristic("notify")
        self.control = FakeCharacteristic("write", "write-without-response")

    def get_characteristic(self, uuid: str) -> FakeCharacteristic | None:
        if uuid == control.STATUS_UUID:
            return self.status
        if uuid == control.CONTROL_UUID:
            return self.control
        return None


class FakeServices:
    def __init__(self) -> None:
        self.service = FakeService()
        self.name = FakeCharacteristic("read")

    def get_service(self, uuid: str) -> FakeService | None:
        return self.service if uuid == control.SERVICE_UUID else None

    def get_characteristic(self, uuid: str) -> FakeCharacteristic | None:
        return self.name if uuid == control.NAME_UUID else None


class ProbeClient(FakeClient):
    def __init__(self) -> None:
        super().__init__()
        self.services = FakeServices()
        self.notify_callback = None
        self.stop_notify_calls = 0

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, _exc_type, _exc, _traceback) -> bool:
        return False

    async def read_gatt_char(self, _uuid: str) -> bytes:
        return control.STATION_NAME.encode("utf-8") + b"\x00\x00"

    async def start_notify(self, _uuid: str, callback) -> None:
        self.notify_callback = callback

    async def stop_notify(self, _uuid: str) -> None:
        self.stop_notify_calls += 1


class StatusFrameTests(unittest.TestCase):
    def test_captured_status_frames_decode_to_expected_values(self) -> None:
        captured = {
            BEFORE_AC_OFF: (0x0E, 0x61, 0, 0, 0x0C39),
            AFTER_AC_OFF: (0x0C, 0x61, 0, 0, 0x0C39),
            DC_ON: (0x0F, 0x61, 0, 0x13, 0x01E5),
            LIGHT_ON: (0x1E, 0x60, 0, 0x13, 0x01AF),
        }
        for frame, expected in captured.items():
            with self.subTest(frame=frame.hex()):
                status = control.decode_status(frame, received_at=12.5)
                self.assertEqual(
                    (
                        status.flags,
                        status.battery_percent,
                        status.input_power_w,
                        status.output_power_w,
                        status.remaining_minutes,
                        status.received_at,
                    ),
                    (*expected, 12.5),
                )

    def test_status_decoder_rejects_header_length_command_and_checksum_errors(self) -> None:
        malformed: dict[str, bytes] = {}

        frame = bytearray(BEFORE_AC_OFF)
        frame[0] ^= 0x01
        malformed["header"] = bytes(frame)

        malformed["length"] = BEFORE_AC_OFF[:-1]
        frame = bytearray(BEFORE_AC_OFF)
        frame[5] = 0x07
        malformed["length byte"] = bytes(frame)

        frame = bytearray(BEFORE_AC_OFF)
        frame[6] = 0x02
        malformed["command"] = bytes(frame)

        frame = bytearray(BEFORE_AC_OFF)
        frame[-1] ^= 0x01
        malformed["checksum"] = bytes(frame)

        for reason, frame in malformed.items():
            with self.subTest(reason=reason, frame=frame.hex()), self.assertRaises(ValueError):
                control.decode_status(frame)

    def test_status_decoder_rejects_wrong_vendor_envelope_with_valid_checksum(self) -> None:
        for index, value in ((2, 0xB0), (3, 0x01), (4, 0x02)):
            frame = bytearray(BEFORE_AC_OFF)
            frame[index] = value
            frame = recompute_checksum(frame)
            with self.subTest(index=index, frame=frame.hex()), self.assertRaises(ValueError):
                control.decode_status(frame)

    def test_invalid_notification_is_dropped_without_replacing_latest(self) -> None:
        async def exercise() -> None:
            stream = control.StatusStream(debug=False)
            with patch.object(control, "emit"):
                stream.callback(None, bytearray(BEFORE_AC_OFF[:-1]))
            notification = stream.queue.get_nowait()
            self.assertIsNone(notification.status)
            self.assertIsNone(stream.latest)

        asyncio.run(exercise())


class ControlProfileTests(unittest.TestCase):
    def test_canonical_vectors_match_archived_captures_and_probe_encoding(self) -> None:
        evidence = Path(__file__).with_name("allpowers_s300_evidence")
        vectors = json.loads((evidence / "regression_vectors.json").read_text())
        checked = 0
        for trip in vectors["round_trips"]:
            if not trip["probe_allowed"]:
                continue
            for operation in trip["operations"]:
                with self.subTest(trip=trip["id"], sequence=operation["tx_seq"]):
                    capture = evidence / operation["capture"]
                    events = {event["seq"]: event for event in map(json.loads, capture.read_text().splitlines())}
                    tx = events[operation["tx_seq"]]
                    self.assertEqual(tx["hex"], operation["command_hex"])
                    self.assertEqual(tx["before"], operation["starting_state"])
                    expected = operation["expected_flags_decimal"]
                    encoded = control.control_frame(control.encode_control_flags(expected))
                    self.assertEqual(encoded.hex(), operation["command_hex"])
                    for notification in operation["first_two_status_notifications"]:
                        self.assertEqual(events[notification["seq"]], notification)
                        self.assertEqual(control.decode_status(bytes.fromhex(notification["hex"])).flags, expected)
                    checked += 1
        self.assertEqual(checked, 6)

    def test_verified_status_profiles_preserve_frequency_and_beep_bits(self) -> None:
        expected_control = {0x0C: 0x18, 0x0D: 0x19, 0x0E: 0x1A, 0x0F: 0x1B,
                            0x1C: 0x38, 0x1D: 0x39, 0x1E: 0x3A, 0x1F: 0x3B}
        for status_flags, control_flags in expected_control.items():
            with self.subTest(status_flags=hex(status_flags)):
                encoded = control.encode_control_flags(status_flags)
                self.assertEqual(encoded, control_flags)
                self.assertTrue(encoded & control.CONTROL_FREQUENCY)
                self.assertTrue(encoded & control.CONTROL_BEEP)

    def test_verified_control_frames_are_exact(self) -> None:
        expected = {
            0x18: "a56500b10101001869",
            0x19: "a56500b10101001968",
            0x1A: "a56500b10101001a6b",
            0x1B: "a56500b10101001b6a",
            0x38: "a56500b10101003849",
            0x39: "a56500b10101003948",
            0x3A: "a56500b10101003a4b",
            0x3B: "a56500b10101003b4a",
        }
        for flags, frame_hex in expected.items():
            with self.subTest(flags=hex(flags)):
                self.assertEqual(control.control_frame(flags), bytes.fromhex(frame_hex))

    def test_supported_overrides_produce_only_verified_combined_states(self) -> None:
        self.assertEqual(control.expected_status_flags(0x0E, "ac", False), 0x0C)
        self.assertEqual(control.expected_status_flags(0x0E, "dc", True), 0x0F)
        self.assertEqual(control.expected_status_flags(0x0E, "light", True), 0x1E)
        self.assertEqual(control.expected_status_flags(0x0C, "ac", True), 0x0E)
        self.assertEqual(control.expected_status_flags(0x0F, "dc", False), 0x0E)
        self.assertEqual(control.expected_status_flags(0x1E, "light", False), 0x0E)

    def test_unknown_and_unsupported_combined_states_are_rejected(self) -> None:
        for flags in (0x00, 0x01, 0x08, 0x10, 0x20, 0x2C, 0x4C, 0x7F, 0x8C):
            with self.subTest(flags=hex(flags)):
                with self.assertRaises(ValueError):
                    control.encode_control_flags(flags)
                with self.assertRaises(ValueError):
                    control.expected_status_flags(flags, "ac", False)

        with self.assertRaises(ValueError):
            control.expected_status_flags(0x0E, "frequency", True)
        with self.assertRaises(ValueError):
            control.control_frame(0x1C)


class AsyncSafetyTests(unittest.IsolatedAsyncioTestCase):
    async def test_exercise_toggles_one_field_from_every_verified_initial_profile(self) -> None:
        for flags in control.STATUS_PROFILES:
            for field, mask in (("ac", 2), ("dc", 1), ("light", 16)):
                with self.subTest(flags=flags, field=field):
                    client = ProbeClient()
                    original = make_status(flags, 10.0)
                    expected = make_status(flags ^ mask, 11.0)
                    send = AsyncMock(return_value=12.0)
                    restore = AsyncMock()
                    with ExitStack() as stack:
                        stack.enter_context(patch.object(control, "resolve_adapter", new=AsyncMock(return_value="hci7")))
                        stack.enter_context(patch.object(control.BleakScanner, "find_device_by_address",
                                                         new=AsyncMock(return_value=DEVICE)))
                        stack.enter_context(patch.object(control, "BleakClient", return_value=client))
                        stack.enter_context(patch.object(control, "wait_fresh",
                                                         new=AsyncMock(side_effect=[original, original])))
                        stack.enter_context(patch.object(control, "send_control", new=send))
                        stack.enter_context(patch.object(control, "wait_matches", new=AsyncMock(return_value=expected)))
                        stack.enter_context(patch.object(control, "restore_exact", new=restore))
                        stack.enter_context(patch.object(control, "emit"))
                        await control.run_probe(SimpleNamespace(exercise=field, seconds=0, hold_seconds=0, debug=False))
                    self.assertEqual(send.await_args.args[3], control.encode_control_flags(flags ^ mask))
                    self.assertEqual(send.await_args.args[4], flags)
                    self.assertEqual(restore.await_args.args[3], original)
                    self.assertEqual(restore.await_args.args[4], flags ^ mask)
                    self.assertEqual(client.stop_notify_calls, 1)

    async def test_send_control_writes_exact_frame_only_for_fresh_expected_snapshot(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)
        now = asyncio.get_running_loop().time()
        stream.latest = make_status(0x0E, now - 0.1)

        with patch.object(control, "emit"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0E)

        self.assertEqual(
            client.writes,
            [(control.CONTROL_UUID, bytes.fromhex("a56500b10101001869"), False)],
        )

    async def test_send_control_refuses_missing_snapshot_without_writing(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)

        with self.assertRaisesRegex(RuntimeError, "status is not fresh"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0C)

        self.assertEqual(client.writes, [])

    async def test_send_control_refuses_stale_snapshot_without_writing(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)
        stale_at = asyncio.get_running_loop().time() - control.STATUS_FRESHNESS - 0.1
        stream.latest = make_status(0x0C, stale_at)

        with self.assertRaisesRegex(RuntimeError, "status is not fresh"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0C)

        self.assertEqual(client.writes, [])

    async def test_send_control_refuses_mismatched_snapshot_without_writing(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)
        stream.latest = make_status(0x0E, asyncio.get_running_loop().time() - 0.1)

        with self.assertRaisesRegex(RuntimeError, "status is not fresh"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0C)

        self.assertEqual(client.writes, [])

    async def test_send_control_refuses_disconnected_client_without_writing(self) -> None:
        client = FakeClient(connected=False)
        stream = control.StatusStream(debug=False)
        stream.latest = make_status(0x0E, asyncio.get_running_loop().time() - 0.1)

        with self.assertRaisesRegex(RuntimeError, "Bluetooth link is disconnected"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0E)

        self.assertEqual(client.writes, [])

    async def test_invalid_unknown_status_clears_snapshot_before_next_write(self) -> None:
        stream = control.StatusStream(debug=False)
        client = FakeClient()
        unknown = bytearray(BEFORE_AC_OFF)
        unknown[7] = 0x80
        unknown = recompute_checksum(unknown)

        with patch.object(control, "emit"):
            stream.callback(None, bytearray(BEFORE_AC_OFF))
            self.assertIsNotNone(stream.latest)
            stream.callback(None, bytearray(unknown))

        self.assertIsNone(stream.latest)
        with self.assertRaisesRegex(RuntimeError, "status is not fresh"):
            await control.send_control(client, DEVICE, stream, 0x18, 0x0E)
        self.assertEqual(client.writes, [])

    async def test_wait_matches_requires_two_fresh_consecutive_postwrite_statuses(self) -> None:
        stream = control.StatusStream(debug=False)
        now = asyncio.get_running_loop().time()
        after = now - 5.0
        stale_at = now - control.STATUS_FRESHNESS - 1.0
        first_match_at = now - 0.4
        contradictory_at = now - 0.3
        fresh_one_at = now - 0.2
        fresh_two_at = now - 0.1

        statuses = (
            # A matching pre-write snapshot is excluded by the write timestamp.
            make_status(0x0C, after - 0.1),
            # One valid match is broken by a contradictory post-write status.
            make_status(0x0C, first_match_at),
            make_status(0x0E, contradictory_at),
            # This matching status is after the write timestamp but too old to confirm it.
            make_status(0x0C, stale_at),
            make_status(0x0C, fresh_one_at),
            make_status(0x0C, fresh_two_at),
        )
        for status in statuses:
            stream.queue.put_nowait(control.Notification(status, status.received_at))

        with patch.object(control, "CONFIRMATION_TIMEOUT", 0.1):
            confirmed = await control.wait_matches(stream, 0x0C, after)

        self.assertEqual(confirmed.flags, 0x0C)
        self.assertEqual(confirmed.received_at, fresh_two_at)

    async def test_wait_matches_times_out_after_only_one_confirmation(self) -> None:
        stream = control.StatusStream(debug=False)
        now = asyncio.get_running_loop().time()
        stream.queue.put_nowait(control.Notification(make_status(0x0C, now), now))

        with patch.object(control, "CONFIRMATION_TIMEOUT", 0.02), self.assertRaises(TimeoutError):
            await control.wait_matches(stream, 0x0C, now - 0.01)

    async def test_restore_exact_refuses_unrelated_state_without_a_write(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)
        original = make_status(0x0E, 10.0)
        unrelated = make_status(0x0F, 11.0)

        with (
            patch.object(control, "wait_fresh", new=AsyncMock(return_value=unrelated)),
            patch.object(control, "send_control", new=AsyncMock()) as send,
            patch.object(control, "emit"),
            self.assertRaisesRegex(RuntimeError, "manual restore required"),
        ):
            await control.restore_exact(client, DEVICE, stream, original, 0x0C)

        send.assert_not_awaited()
        self.assertEqual(client.writes, [])

    async def test_restore_exact_refuses_disconnected_link_without_a_write(self) -> None:
        client = FakeClient(connected=False)
        stream = control.StatusStream(debug=False)
        original = make_status(0x0E, 10.0)

        with (
            patch.object(control, "wait_fresh", new=AsyncMock()) as wait,
            patch.object(control, "emit"),
            self.assertRaisesRegex(RuntimeError, "Bluetooth link is disconnected"),
        ):
            await control.restore_exact(client, DEVICE, stream, original, 0x0C)

        wait.assert_not_awaited()
        self.assertEqual(client.writes, [])

    async def test_restore_exact_skips_write_when_original_state_is_already_present(self) -> None:
        client = FakeClient()
        stream = control.StatusStream(debug=False)
        original = make_status(0x0E, 10.0)

        with (
            patch.object(control, "wait_fresh", new=AsyncMock(return_value=original)),
            patch.object(control, "send_control", new=AsyncMock()) as send,
            patch.object(control, "emit"),
        ):
            await control.restore_exact(client, DEVICE, stream, original, 0x0C)

        send.assert_not_awaited()
        self.assertEqual(client.writes, [])

    async def test_restore_exact_writes_1a_for_each_verified_test_state(self) -> None:
        for expected_test in (0x0C, 0x0F, 0x1E):
            with self.subTest(expected_test=hex(expected_test)):
                client = FakeClient()
                stream = control.StatusStream(debug=False)
                now = asyncio.get_running_loop().time()
                original = make_status(0x0E, now - 0.1)
                test_state = make_status(expected_test, now - 0.1)
                stream.latest = test_state

                async def write_and_confirm(
                    uuid: str,
                    payload: bytes,
                    response: bool = False,
                    *,
                    client_ref=client,
                    stream_ref=stream,
                ) -> None:
                    client_ref.writes.append((uuid, bytes(payload), response))
                    await asyncio.sleep(0)
                    for _ in range(2):
                        timestamp = asyncio.get_running_loop().time()
                        status = make_status(0x0E, timestamp)
                        stream_ref.queue.put_nowait(control.Notification(status, timestamp))

                client.write_gatt_char = write_and_confirm

                with (
                    patch.object(control, "wait_fresh", new=AsyncMock(return_value=test_state)),
                    patch.object(control, "emit"),
                ):
                    await control.restore_exact(client, DEVICE, stream, original, expected_test)

                self.assertEqual(
                    client.writes,
                    [(control.CONTROL_UUID, bytes.fromhex("a56500b10101001a6b"), False)],
                )
                self.assertTrue(stream.queue.empty())

    async def test_run_probe_restores_after_failure_and_cancellation(self) -> None:
        args = SimpleNamespace(exercise="ac", seconds=0.0, hold_seconds=1.0, debug=False)

        for cancellation in (False, True):
            with self.subTest(cancellation=cancellation):
                client = ProbeClient()
                original = make_status(0x0E, 10.0)
                expected = make_status(0x0C, 11.0)
                restore = AsyncMock()

                async def fail_during_hold(_seconds: float, *, cancel=cancellation) -> None:
                    if cancel:
                        raise asyncio.CancelledError()
                    raise RuntimeError("injected hold failure")

                with ExitStack() as stack:
                    stack.enter_context(
                        patch.object(control, "resolve_adapter", new=AsyncMock(return_value="hci7"))
                    )
                    stack.enter_context(
                        patch.object(
                            control.BleakScanner,
                            "find_device_by_address",
                            new=AsyncMock(return_value=DEVICE),
                        )
                    )
                    stack.enter_context(patch.object(control, "BleakClient", return_value=client))
                    stack.enter_context(
                        patch.object(
                            control,
                            "wait_fresh",
                            new=AsyncMock(side_effect=[original, original]),
                        )
                    )
                    stack.enter_context(
                        patch.object(control, "send_control", new=AsyncMock(return_value=12.0))
                    )
                    stack.enter_context(
                        patch.object(control, "wait_matches", new=AsyncMock(return_value=expected))
                    )
                    stack.enter_context(patch.object(control, "restore_exact", new=restore))
                    stack.enter_context(patch.object(control, "emit"))
                    stack.enter_context(patch.object(control.asyncio, "sleep", new=fail_during_hold))

                    if cancellation:
                        with self.assertRaises(asyncio.CancelledError):
                            await control.run_probe(args)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "injected hold failure"):
                            await control.run_probe(args)

                restore.assert_awaited_once()
                restore_args = restore.await_args.args
                self.assertIs(restore_args[0], client)
                self.assertIs(restore_args[1], DEVICE)
                self.assertEqual(restore_args[3].flags, 0x0E)
                self.assertEqual(restore_args[4], 0x0C)
                self.assertEqual(client.stop_notify_calls, 1)

    async def test_run_probe_bounds_a_hung_failure_restoration(self) -> None:
        args = SimpleNamespace(exercise="ac", seconds=0.0, hold_seconds=1.0, debug=False)
        client = ProbeClient()
        original = make_status(0x0E, 10.0)
        expected = make_status(0x0C, 11.0)
        emissions: list[tuple[dict, bool]] = []

        async def fail_during_hold(_seconds: float) -> None:
            raise RuntimeError("injected hold failure")

        async def hung_restore(*_args) -> None:
            await asyncio.Event().wait()

        def collect(payload: dict, *, error: bool = False) -> None:
            emissions.append((payload, error))

        with ExitStack() as stack:
            stack.enter_context(
                patch.object(control, "resolve_adapter", new=AsyncMock(return_value="hci7"))
            )
            stack.enter_context(
                patch.object(
                    control.BleakScanner,
                    "find_device_by_address",
                    new=AsyncMock(return_value=DEVICE),
                )
            )
            stack.enter_context(patch.object(control, "BleakClient", return_value=client))
            stack.enter_context(
                patch.object(control, "wait_fresh", new=AsyncMock(side_effect=[original, original]))
            )
            stack.enter_context(patch.object(control, "send_control", new=AsyncMock(return_value=12.0)))
            stack.enter_context(patch.object(control, "wait_matches", new=AsyncMock(return_value=expected)))
            stack.enter_context(patch.object(control, "restore_exact", new=hung_restore))
            stack.enter_context(patch.object(control, "emit", side_effect=collect))
            stack.enter_context(patch.object(control.asyncio, "sleep", new=fail_during_hold))
            stack.enter_context(patch.object(control, "CLEANUP_TIMEOUT", 0.01))

            with self.assertRaisesRegex(RuntimeError, "injected hold failure"):
                await control.run_probe(args)

        manual = [payload for payload, error in emissions if payload.get("event") == "manual_restore_required"]
        self.assertEqual(len(manual), 1)
        self.assertEqual(manual[0]["expected_test_flags"], 0x0C)
        self.assertEqual(client.stop_notify_calls, 1)

    async def test_run_probe_reports_manual_restore_after_disconnect_without_writing(self) -> None:
        args = SimpleNamespace(exercise="ac", seconds=0.0, hold_seconds=1.0, debug=False)
        client = ProbeClient()
        original = make_status(0x0E, 10.0)
        expected = make_status(0x0C, 11.0)
        emissions: list[tuple[dict, bool]] = []

        async def disconnect_during_hold(_seconds: float) -> None:
            client.is_connected = False
            raise RuntimeError("link dropped")

        def collect(payload: dict, *, error: bool = False) -> None:
            emissions.append((payload, error))

        with ExitStack() as stack:
            stack.enter_context(
                patch.object(control, "resolve_adapter", new=AsyncMock(return_value="hci7"))
            )
            stack.enter_context(
                patch.object(
                    control.BleakScanner,
                    "find_device_by_address",
                    new=AsyncMock(return_value=DEVICE),
                )
            )
            stack.enter_context(patch.object(control, "BleakClient", return_value=client))
            stack.enter_context(
                patch.object(control, "wait_fresh", new=AsyncMock(side_effect=[original, original]))
            )
            stack.enter_context(patch.object(control, "send_control", new=AsyncMock(return_value=12.0)))
            stack.enter_context(patch.object(control, "wait_matches", new=AsyncMock(return_value=expected)))
            stack.enter_context(patch.object(control, "emit", side_effect=collect))
            stack.enter_context(patch.object(control.asyncio, "sleep", new=disconnect_during_hold))

            with self.assertRaisesRegex(RuntimeError, "link dropped"):
                await control.run_probe(args)

        manual = [
            (payload, error)
            for payload, error in emissions
            if payload.get("event") == "manual_restore_required"
        ]
        self.assertEqual(len(manual), 1)
        self.assertTrue(manual[0][1])
        self.assertEqual(client.writes, [])
        self.assertEqual(client.stop_notify_calls, 1)


if __name__ == "__main__":
    unittest.main()
