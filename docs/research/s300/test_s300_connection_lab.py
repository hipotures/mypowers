"""Offline guards against accidental overlap of connection experiments."""

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from s300_connection_lab import Lab


class FakeClient:
    def __init__(self, *, connected=True, disconnect_error=None, connect_error=None):
        self.is_connected = connected
        self.disconnect_error = disconnect_error
        self.connect_error = connect_error
        self.disconnect_calls = 0

    async def disconnect(self):
        self.disconnect_calls += 1
        if self.disconnect_error:
            raise self.disconnect_error
        self.is_connected = False

    async def connect(self):
        if self.connect_error:
            raise self.connect_error


class CleanupTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.lab = Lab(Path(self.directory.name) / "capture.jsonl")
        self.lab.device = SimpleNamespace(address="2A:02:01:48:6B:D0", details={})
        self.lab.adapter = "hci2"

    async def asyncTearDown(self):
        self.lab.file.close()
        self.directory.cleanup()

    async def test_failed_disconnect_retains_client_and_blocks_next_connect(self):
        client = FakeClient(disconnect_error=RuntimeError("cleanup failed"))
        self.lab.client = client
        with self.assertRaisesRegex(RuntimeError, "cleanup failed"):
            await self.lab.disconnect()
        self.assertIs(self.lab.client, client)
        with patch("s300_connection_lab.BleakClient") as constructor:
            with self.assertRaisesRegex(RuntimeError, "Disconnect the current"):
                await self.lab.connect("fff1")
            constructor.assert_not_called()

    async def test_duplicate_connect_does_not_disconnect_existing_link(self):
        client = FakeClient()
        self.lab.client = client
        await self.lab.command_run("connect")
        self.assertIs(self.lab.client, client)
        self.assertEqual(client.disconnect_calls, 0)

    async def test_invalid_channel_does_not_disconnect_existing_link(self):
        client = FakeClient()
        self.lab.client = client
        await self.lab.command_run("connect unsupported")
        self.assertIs(self.lab.client, client)
        self.assertEqual(client.disconnect_calls, 0)

    async def test_failed_new_connection_preserves_error_and_cleans_only_new_client(self):
        client = FakeClient(connected=False, connect_error=TimeoutError("primary failure"))
        with (
            patch("s300_connection_lab.BleakClient", return_value=client),
            self.assertRaisesRegex(TimeoutError, "primary failure"),
        ):
            await self.lab.connect("fff1")
        self.assertIsNone(self.lab.client)
        self.assertEqual(client.disconnect_calls, 1)


if __name__ == "__main__":
    unittest.main()
