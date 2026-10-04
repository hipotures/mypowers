"""Bounded incremental keyboard, SGR mouse and bracketed-paste decoder."""

import re
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class InputEvent:
    kind: str
    key: str = ""
    x: int = 0
    y: int = 0
    button: int = 0


MOUSE = re.compile(rb"\x1b\[<(\d{1,3});(\d{1,5});(\d{1,5})([Mm])")
KEYS = {
    b"\x1b[Z": "backtab",
    b"\x1b[A": "up",
    b"\x1b[B": "down",
    b"\x1b[5~": "pageup",
    b"\x1b[6~": "pagedown",
}


class Decoder:
    def __init__(self) -> None:
        self.buffer = bytearray()
        self.paste = False

    def feed(self, raw: bytes) -> list[InputEvent]:
        self.buffer.extend(raw)
        if len(self.buffer) > 4096:
            self.buffer.clear()
            return []
        events: list[InputEvent] = []
        while self.buffer:
            if self.paste:
                end = self.buffer.find(b"\x1b[201~")
                if end < 0:
                    self.buffer[:] = self.buffer[-5:]
                    break
                del self.buffer[: end + 6]
                self.paste = False
                continue
            if self.buffer.startswith(b"\x1b[200~"):
                del self.buffer[:6]
                self.paste = True
                continue
            match = MOUSE.match(self.buffer)
            if match:
                button, x, y = (int(match[index]) for index in (1, 2, 3))
                kind = "scroll" if button & 64 else "press" if match[4] == b"M" else "release"
                events.append(InputEvent(kind, x=x, y=y, button=button))
                del self.buffer[: match.end()]
                continue
            matched = False
            for code, named_key in KEYS.items():
                if self.buffer.startswith(code):
                    events.append(InputEvent("key", named_key))
                    del self.buffer[: len(code)]
                    matched = True
                    break
            if matched:
                continue
            if self.buffer[0] == 27:
                candidates = [b"\x1b[200~", b"\x1b[201~", b"\x1b[<", *KEYS]
                if any(code.startswith(self.buffer) for code in candidates) or (
                    self.buffer.startswith(b"\x1b[<")
                    and len(self.buffer) < 32
                    and self.buffer[-1] not in b"Mm"
                ):
                    break
                # Discard the entire unsupported CSI, rather than interpreting its suffix as keys.
                if self.buffer.startswith(b"\x1b["):
                    terminator = next(
                        (i for i in range(2, len(self.buffer)) if 64 <= self.buffer[i] <= 126), None
                    )
                    if terminator is None:
                        if len(self.buffer) < 32:
                            break
                        self.buffer.clear()
                    else:
                        del self.buffer[: terminator + 1]
                else:
                    del self.buffer[: min(2, len(self.buffer))]
                continue
            char = self.buffer.pop(0)
            key = {9: "tab", 10: "enter", 13: "enter", 32: "space", 3: "quit", 26: "quit"}.get(char)
            if key is None and 33 <= char <= 126:
                key = chr(char)
            if key:
                events.append(InputEvent("key", key))
        return events
