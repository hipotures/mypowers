import json
from functools import reduce
from operator import xor
from pathlib import Path

import pytest
from conftest import frame

from mypowers.contracts import Output
from mypowers.protocol import PROFILES, decode, encode, target_flags

FRAMES = {
    12: "a56500b10101001869",
    13: "a56500b10101001968",
    14: "a56500b10101001a6b",
    15: "a56500b10101001b6a",
    28: "a56500b10101003849",
    29: "a56500b10101003948",
    30: "a56500b10101003a4b",
    31: "a56500b10101003b4a",
}


@pytest.mark.parametrize("flags", FRAMES)
def test_exact_all_eight(flags):
    assert encode(flags).hex() == FRAMES[flags]
    assert reduce(xor, encode(flags), 0) == 0


@pytest.mark.parametrize("source", PROFILES)
@pytest.mark.parametrize("output,mask", [(Output.AC, 2), (Output.DC, 1), (Output.LIGHT, 16)])
def test_all_24_directed_edges_and_noops(source, output, mask):
    target = target_flags(source, output, not bool(source & mask))
    assert target == source ^ mask
    assert source & ~mask == target & ~mask
    assert encode(target).hex() == FRAMES[target]
    assert target_flags(source, output, bool(source & mask)) == source


@pytest.mark.parametrize("value", [0, 1, 8, 32, 44, 76, 128, 256, -1])
def test_unqualified_write_profiles(value):
    with pytest.raises(ValueError):
        encode(value)
    with pytest.raises(ValueError):
        target_flags(value, Output.LIGHT, True)


@pytest.mark.parametrize("index", [0, 1, 2, 3, 4, 5, 6, 15])
def test_invalid_envelope(index):
    raw = bytearray(frame())
    raw[index] ^= 1
    if index != 15:
        raw[-1] = reduce(xor, raw[:-1], 0)
    with pytest.raises(ValueError):
        decode(bytes(raw))


@pytest.mark.parametrize(
    "raw", [b"", frame()[:-1], frame() + b"x", frame() + frame(), frame(128), frame(battery=101)]
)
def test_invalid_shape_or_range(raw):
    with pytest.raises(ValueError):
        decode(raw)


@pytest.mark.parametrize("value", [0, 65535])
def test_zero_and_uint16_boundaries(value):
    result = decode(frame(input_w=value, output_w=value, minutes=value))
    assert (result.input_w, result.output_w, result.minutes) == (value, value, value)


def test_golden_and_diagnostic_lower_profiles():
    result = decode(bytes.fromhex("a565b1000108010e63003700190190ab"))
    assert (result.battery, result.input_w, result.output_w, result.minutes, result.flags) == (
        99,
        55,
        25,
        400,
        14,
    )
    assert decode(frame(8)).flags == 8
    assert encode(28)[7] == 56


def test_captured_fixtures():
    data = json.loads((Path(__file__).parents[1] / "fixtures/s300/status.json").read_text())
    for record in data["frames"]:
        assert decode(bytes.fromhex(record["frame"])).flags == record["flags"]


@pytest.mark.parametrize(
    "value",
    [b"", bytes.fromhex("a56500b10101000071"), bytes.fromhex("a56500b10101001868"), frame()],
)
def test_transport_allowlist_rejects_raw_escape_hatch(value):
    from mypowers.protocol import validate_control

    with pytest.raises(ValueError):
        validate_control(value)
