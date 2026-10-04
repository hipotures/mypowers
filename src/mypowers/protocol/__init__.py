"""Pure exact-unit protocol, independently derived from the archived qualification."""

from dataclasses import dataclass
from functools import reduce
from operator import xor

from mypowers.contracts import Output

STATION_ADDRESS = "2A:02:01:48:6B:D0"
STATION_NAME = "AP S300 V2.0"
PROFILE = "s300-v2-qualified-2026-10-04"
SERVICE_UUID = "0000fff0-0000-1000-8000-00805f9b34fb"
STATUS_UUID = "0000fff1-0000-1000-8000-00805f9b34fb"
CONTROL_UUID = "0000fff2-0000-1000-8000-00805f9b34fb"
NAME_UUID = "00002a00-0000-1000-8000-00805f9b34fb"
PROFILES = frozenset((12, 13, 14, 15, 28, 29, 30, 31))
TX_PROFILES = frozenset((24, 25, 26, 27, 56, 57, 58, 59))
MASKS = {Output.AC: 2, Output.DC: 1, Output.LIGHT: 16}


@dataclass(frozen=True, slots=True)
class Reading:
    flags: int
    battery: int
    input_w: int
    output_w: int
    minutes: int


def decode(frame: bytes) -> Reading:
    if len(frame) != 16 or frame[:7] != bytes.fromhex("a565b100010801"):
        raise ValueError("Invalid qualified status envelope or length.")
    if reduce(xor, frame, 0):
        raise ValueError("Invalid XOR checksum.")
    if frame[8] > 100 or frame[7] & 128:
        raise ValueError("Battery or higher status flags outside supported range.")
    return Reading(
        frame[7],
        frame[8],
        int.from_bytes(frame[9:11], "big"),
        int.from_bytes(frame[11:13], "big"),
        int.from_bytes(frame[13:15], "big"),
    )


def target_flags(source: int, output: Output, enabled: bool) -> int:
    if source not in PROFILES:
        raise ValueError("Complete state is outside the qualified profile.")
    mask = MASKS[output]
    return source | mask if enabled else source & ~mask


def encode(flags: int) -> bytes:
    if flags not in PROFILES:
        raise ValueError("Complete state is outside the qualified profile.")
    tx = (flags & 3) | ((flags & 12) << 1) | ((flags & 16) << 1)
    if tx not in TX_PROFILES:
        raise ValueError("Control is outside the qualified allowlist.")
    body = bytes.fromhex("a56500b1010100") + bytes([tx])
    return body + bytes([reduce(xor, body, 0)])


def validate_control(frame: bytes) -> None:
    if (
        len(frame) != 9
        or frame[:7] != bytes.fromhex("a56500b1010100")
        or frame[7] not in TX_PROFILES
        or reduce(xor, frame, 0)
    ):
        raise ValueError("Control frame is outside the qualified allowlist.")
