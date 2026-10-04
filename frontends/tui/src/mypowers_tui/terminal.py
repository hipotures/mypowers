"""Own Linux terminal settings and restore every mode on supported exit paths."""

import os
import sys
import termios
import tty
from types import TracebackType
from typing import Any

ENABLE_PASTE = "\x1b[?2004h"
ENABLE_MOUSE = "\x1b[?1000h\x1b[?1006h"
RESTORE = "\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?1049l"


class Terminal:
    def __init__(self, mouse: bool = True):
        self.mouse = mouse
        self.fd = sys.stdin.fileno()
        self.original: list[Any] | None = None

    def __enter__(self) -> "Terminal":
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise ValueError("Interactive TUI needs a terminal. Use 'mypowers status' for pipes.")
        self.original = termios.tcgetattr(self.fd)
        try:
            tty.setcbreak(self.fd)
            current = termios.tcgetattr(self.fd)
            # Map Ctrl+Z to clean exit instead of leaving terminal modes active during suspension.
            current[6][termios.VSUSP] = b"\0"
            termios.tcsetattr(self.fd, termios.TCSANOW, current)
            os.write(
                sys.stdout.fileno(), (ENABLE_PASTE + (ENABLE_MOUSE if self.mouse else "")).encode()
            )
        except BaseException:
            self.restore()
            raise
        return self

    def restore(self) -> None:
        try:
            if self.original is not None:
                termios.tcsetattr(self.fd, termios.TCSANOW, self.original)
        finally:
            if self.original is not None:
                try:
                    os.write(sys.stdout.fileno(), RESTORE.encode())
                except OSError:
                    pass
                self.original = None

    def __exit__(
        self,
        kind: type[BaseException] | None,
        error: BaseException | None,
        trace: TracebackType | None,
    ) -> None:
        self.restore()
