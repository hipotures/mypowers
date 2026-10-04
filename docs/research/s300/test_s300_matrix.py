"""Offline encoding and captured-evidence checks; never opens Bluetooth."""

import asyncio
import json
import unittest
from pathlib import Path
from unittest.mock import patch

import control_allpowers as control
import s300_matrix_validation as matrix

EVIDENCE = Path(__file__).with_name("allpowers_s300_evidence") / "matrix"
EXPECTED = {
    0x0C: "a56500b10101001869", 0x0D: "a56500b10101001968",
    0x0E: "a56500b10101001a6b", 0x0F: "a56500b10101001b6a",
    0x1C: "a56500b10101003849", 0x1D: "a56500b10101003948",
    0x1E: "a56500b10101003a4b", 0x1F: "a56500b10101003b4a",
}


class MatrixTests(unittest.TestCase):
    def test_all_eight_exact_frames_and_allowlists(self):
        self.assertEqual(control.STATUS_PROFILES, set(EXPECTED))
        self.assertEqual(control.CONTROL_ALLOWLIST, {bytes.fromhex(x)[7] for x in EXPECTED.values()})
        for rx, hex_frame in EXPECTED.items():
            with self.subTest(rx=rx):
                expected = bytes.fromhex(hex_frame)
                self.assertEqual(control.control_frame(control.encode_control_flags(rx)), expected)
                self.assertEqual(matrix.candidate_frame(rx), expected)
                checksum = 0
                for value in expected[:-1]:
                    checksum ^= value
                self.assertEqual(expected[-1], checksum)

    def test_every_single_switch_encoding_preserves_other_fields(self):
        # All 24 directed edges are encoding checks, not 24 hardware trials.
        for source in EXPECTED:
            for field, mask in (("ac", 2), ("dc", 1), ("light", 16)):
                target = source ^ mask
                with self.subTest(source=source, field=field):
                    enabled = not bool(source & mask)
                    self.assertEqual(control.expected_status_flags(source, field, enabled), target)
                    encoded = control.encode_control_flags(source, **{field: enabled})
                    self.assertEqual(control.control_frame(encoded).hex(), EXPECTED[target])
                    self.assertEqual(source & ~mask, target & ~mask)
                    self.assertEqual(target & 12, 12)

    def test_captured_transitions_match_raw_log_and_two_consecutive_confirmations(self):
        checked = 0
        for session in ("first", "repeat", "manual-camera"):
            vectors = json.loads((EVIDENCE / session / "matrix_vectors.json").read_text())
            records = [json.loads(line) for line in
                       (EVIDENCE / session / "session.jsonl").read_text().splitlines()]
            events = {record["seq"]: record for record in records}
            self.assertTrue(vectors["success"])
            self.assertEqual(len(vectors["transitions"]), 8)
            self.assertEqual({v["expected_rx_flags"] for v in vectors["transitions"]}, set(EXPECTED))
            for vector in vectors["transitions"]:
                with self.subTest(session=session, tx=vector["tx_seq"]):
                    tx = events[vector["tx_seq"]]
                    target = vector["expected_rx_flags"]
                    self.assertEqual(vector["tx_frame"], EXPECTED[target])
                    self.assertEqual(tx["frame"], vector["tx_frame"])
                    self.assertEqual(tx["before"], vector["starting_status"])
                    self.assertEqual(tx["tx_flags"], vector["tx_flags"])
                    self.assertEqual(tx["checksum"], vector["checksum"])
                    self.assertFalse(tx["response"])
                    self.assertLessEqual(tx["monotonic"] - tx["before"]["monotonic"], 3)
                    source = tx["before"]["status"]["flags"]
                    self.assertEqual((source ^ target).bit_count(), 1)
                    self.assertEqual(source & 12, target & 12)
                    self.assertEqual(matrix.state(source), vector["source_state"])
                    self.assertEqual(matrix.state(target), vector["target_state"])
                    post = [r for r in records if r["event"] == "rx"
                            and r["monotonic"] > tx["monotonic"]]
                    first_match = next(i for i, r in enumerate(post) if r["status"]["flags"] == target)
                    pair = post[first_match:first_match + 2]
                    self.assertEqual(pair, vector["first_two_post_write_status_frames"])
                    for rx in pair:
                        self.assertEqual(events[rx["seq"]], rx)
                        self.assertEqual(control.decode_status(bytes.fromhex(rx["frame"])).flags, target)
                    self.assertAlmostEqual(vector["first_response_latency_s"],
                                           pair[0]["monotonic"] - tx["monotonic"])
                    self.assertAlmostEqual(vector["second_confirmation_latency_s"],
                                           pair[1]["monotonic"] - tx["monotonic"])
                    checked += 1
        self.assertEqual(checked, 24)

    def test_both_sessions_restore_original_state_and_disconnect(self):
        for session in ("first", "repeat", "manual-camera"):
            vectors = json.loads((EVIDENCE / session / "matrix_vectors.json").read_text())
            events = [json.loads(line) for line in
                      (EVIDENCE / session / "session.jsonl").read_text().splitlines()]
            original = vectors["initial_status"]["status"]["flags"]
            self.assertEqual(original, 12)
            self.assertEqual(vectors["final_status"]["status"]["flags"], original)
            self.assertEqual(vectors["transitions"][-1]["expected_rx_flags"], original)
            self.assertEqual(events[-2]["event"], "disconnected")
            self.assertFalse(events[-2]["connected"])
            self.assertTrue(events[-1]["success"])
            self.assertFalse(any(e["event"] in {"rx_invalid", "stopped", "manual_restore_required"}
                                 for e in events))

    def test_experimental_encoder_refuses_unknown_profiles(self):
        for rx in (0, 8, 0x2C, 0x4C, 0x8C):
            with self.subTest(rx=rx), self.assertRaises(ValueError):
                matrix.candidate_frame(rx)


class MatrixSafetyTests(unittest.IsolatedAsyncioTestCase):
    async def test_unexpected_notification_invalidates_snapshot_and_stops(self):
        capture = object.__new__(matrix.Capture)
        capture.latest = {"previous": True}
        capture.queue = asyncio.Queue()
        capture.fatal = None
        capture.allowed_flags = {0x0C}
        # Checksum-correct but uncommanded AC change must not be ignored.
        raw = bytearray.fromhex("a565b1000108010e61000000000c3923")
        with patch.object(capture, "log", side_effect=lambda event, **kw: {"event": event, **kw}):
            capture.callback(None, raw)
        self.assertIsNone(capture.latest)
        self.assertIn("unexpected status", capture.fatal)
        with self.assertRaisesRegex(RuntimeError, "unexpected status"):
            await capture.fresh(asyncio.get_running_loop().time())


if __name__ == "__main__":
    unittest.main()
