"""Explicit development transport; never an automatic BLE fallback."""

import asyncio
from collections.abc import Callable
from functools import reduce
from operator import xor


class SimulatedTransport:
    def __init__(self, flags: int = 12, interval: float = 0.3):
        self.adapter_id = "simulated"
        self.flags, self.interval = flags, interval
        self.task: asyncio.Task[None] | None = None
        self.active = False
        self.writes: list[bytes] = []
        self.callback: Callable[[bytes], None] = lambda _: None

    async def acquire(self, phase: Callable[[str], None]) -> None:
        phase("connecting")
        self.active = True

    async def subscribe(self, callback: Callable[[bytes], None]) -> None:
        self.callback = callback
        self.task = asyncio.create_task(self.emit())

    def frame(self) -> bytes:
        body = (
            bytes.fromhex("a565b100010801")
            + bytes([self.flags, 88])
            + bytes.fromhex("000000001ed7")
        )
        return body + bytes([reduce(xor, body, 0)])

    async def emit(self) -> None:
        while self.active:
            self.callback(self.frame())
            await asyncio.sleep(self.interval)

    async def write(self, frame: bytes) -> None:
        self.writes.append(frame)
        tx = frame[7]
        self.flags = (tx & 3) | ((tx & 24) >> 1) | ((tx & 32) >> 1)

    def connected(self) -> bool:
        return self.active

    async def cleanup(self) -> None:
        self.active = False
        if self.task:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
            self.task = None
