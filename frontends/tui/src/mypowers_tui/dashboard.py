"""Adaptive instrument-style rendering, hitboxes and timestamp-based power trends."""

import time
from collections import deque
from dataclasses import dataclass
from typing import Any

from rich.console import Group
from rich.panel import Panel
from rich.text import Text

from mypowers.contracts import Sample, Status, safe_text

BLUE = "bright_blue"
CYAN = "bright_cyan"
MUTED = "dim"


@dataclass(frozen=True, slots=True)
class Hitbox:
    action: str
    x1: int
    x2: int
    y: int

    def contains(self, x: int, y: int) -> bool:
        return self.x1 <= x <= self.x2 and self.y == y


class Dashboard:
    def __init__(self) -> None:
        self.status: Status | None = None
        self.received = 0.0
        self.server_connected = False
        self.view = "dashboard"
        self.focus = 0
        self.pending: str | None = None
        self.notice = "Connecting to server..."
        self.hits: list[Hitbox] = []
        self.generation = 0
        self.layout_key: tuple[Any, ...] | None = None
        self.trends: deque[tuple[float, Sample]] = deque(maxlen=512)
        self.logs: deque[dict[str, Any]] = deque(maxlen=1000)
        self.scroll = 0
        self.level = "INFO"
        self.last_sequence: tuple[str, int] | None = None

    def update(self, status: Status) -> None:
        previous = self.status
        if previous and previous.server_instance_id != status.server_instance_id:
            self.trends.clear()
        self.status, self.received, self.server_connected = status, time.monotonic(), True
        sample = status.telemetry.sample
        identity = (status.server_instance_id, sample.sequence) if sample else None
        if sample and status.telemetry.state == "live" and identity != self.last_sequence:
            self.trends.append((time.monotonic(), sample))
            self.last_sequence = identity
        while self.trends and time.monotonic() - self.trends[0][0] > 120:
            self.trends.popleft()

    def live(self) -> bool:
        status = self.status
        if status is None or not self.server_connected or status.telemetry.age_seconds is None:
            return False
        age = status.telemetry.age_seconds + time.monotonic() - self.received
        return status.telemetry.state == "live" and age < 3

    def allowed(self) -> bool:
        return (
            self.live()
            and self.status is not None
            and self.status.controls.allowed
            and not self.pending
        )

    def trend(self, output: bool, columns: int) -> str:
        if not self.trends:
            return "No recent samples"
        start = time.monotonic() - 120
        bins: list[list[tuple[int, str]]] = [[] for _ in range(columns)]
        for instant, sample in self.trends:
            index = min(columns - 1, int((instant - start) / 120 * columns))
            if index >= 0:
                bins[index].append(
                    (sample.output_power_w if output else sample.input_power_w, sample.segment_id)
                )
        maximum = max((point[0] for values in bins for point in values), default=1) or 1
        glyphs = "_▁▂▃▄▅▆▇█"
        rendered = []
        previous_segment: str | None = None
        for values in bins:
            if not values:
                rendered.append(" ")
                previous_segment = None
                continue
            value, segment = values[-1]
            if (
                len({point[1] for point in values}) > 1
                or previous_segment
                and previous_segment != segment
            ):
                rendered.append("|")
            else:
                rendered.append(glyphs[min(8, round(value / maximum * 8))])
            previous_segment = segment
        return "".join(rendered)

    def render(self, width: int, height: int) -> Panel:
        status = self.status
        key = (
            width,
            height,
            self.view,
            status.server_instance_id if status else None,
            status.controls.outputs_revision if status else None,
            self.allowed(),
            self.pending,
        )
        if key != self.layout_key:
            self.generation += 1
            self.layout_key = key
        self.hits = []
        if width < 40 or height < 12:
            return Panel(
                Text("Resize terminal to at least 40 x 12.\nPress q to quit."), title="MYPOWERS"
            )
        inner = width - 4
        rows: list[Text] = []

        def line(value: str = "", style: str = "") -> None:
            rows.append(Text(safe_text(value), style=style, overflow="ellipsis", no_wrap=True))

        def buttons(entries: list[tuple[str, str]], enabled: bool = True) -> None:
            row = Text(no_wrap=True, overflow="ellipsis")
            for action, label in entries:
                index = len(self.hits)
                shown = "[" + label + "]"
                style = "reverse bold" if index == self.focus else "bold " + BLUE
                if not enabled:
                    style = MUTED
                self.hits.append(
                    Hitbox(action, 3 + len(row), 2 + len(row) + len(shown), len(rows) + 2)
                )
                row.append(shown, style=style).append("  ")
            rows.append(row)

        phase = status.connection.phase.upper() if status else "UNKNOWN"
        server = "CONNECTED" if self.server_connected else "LOST"
        freshness = (
            "LIVE"
            if self.live()
            else "LAST KNOWN"
            if status and status.telemetry.sample
            else "UNKNOWN"
        )
        line(f"SERVER {server}   STATION {phase}   DATA {freshness}", CYAN)
        sample = status.telemetry.sample if status else None
        if self.view == "help":
            for text in (
                "Tab / Shift-Tab: focus    Enter / Space: activate",
                "Mouse: click controls, wheel scrolls logs",
                "d: dashboard   l: logs   ?: help   f: log level",
                "q / Ctrl+C / Ctrl+Z: clean exit",
                "Changes require live data and server confirmation.",
                "Pending targets are separate from observed state.",
            ):
                line(text)
        elif self.view == "logs":
            line(f"LOGS  minimum {self.level}   f: change filter   Up/Down: scroll", BLUE)
            records = list(self.logs)
            from logging import getLevelNamesMapping

            levels = getLevelNamesMapping()
            filtered = [
                r for r in records if levels.get(r.get("level", "INFO"), 20) >= levels[self.level]
            ]
            available = max(1, height - 8)
            stop = max(0, len(filtered) - self.scroll)
            for record in filtered[max(0, stop - available) : stop]:
                line(
                    f"{record.get('timestamp', '')} {record.get('level', '')} "
                    f"{record.get('message', '')}"
                )
        else:
            compact = height < 20
            if not compact:
                line(status.device["name"] if status else "ALLPOWERS S300", MUTED)
                line()
            line(f"BATTERY   {sample.battery_percent if sample else '--'}%", "bold bright_white")
            if not compact:
                bar_width = min(50, inner - 2)
                filled = round((sample.battery_percent if sample else 0) / 100 * bar_width)
                line("[" + "=" * filled + "." * (bar_width - filled) + "]", BLUE)
                line()
            input_value = f"{sample.input_power_w} W" if sample else "-- W"
            output_value = f"{sample.output_power_w} W" if sample else "-- W"
            half = max(12, inner // 2)
            if compact:
                line(f"INPUT {input_value}   OUTPUT {output_value}", "bold " + CYAN)
            else:
                line("INPUT".ljust(half) + "OUTPUT", MUTED)
                line(input_value.ljust(half) + output_value, "bold " + CYAN)
            if height >= 24:
                line(self.trend(False, half - 2).ljust(half) + self.trend(True, half - 2), BLUE)
                line()
            states = [
                (name, ("On" if getattr(sample, name + "_enabled") else "Off") if sample else "?")
                for name in ("ac", "dc", "light")
            ]
            buttons(
                [
                    (name, ("LAMPS" if name == "light" else name.upper()) + " " + value)
                    for name, value in states
                ],
                enabled=self.allowed(),
            )
            if self.pending and not compact:
                line("PENDING " + self.pending, "yellow")
            if sample:
                age = (
                    (status.telemetry.age_seconds or 0) + time.monotonic() - self.received
                    if status
                    else 0
                )
                estimate = f"{sample.remaining_minutes // 60}h {sample.remaining_minutes % 60:02d}m"
                line(
                    f"{'Estimate' if compact else 'Station estimate'} {estimate}"
                    f"   Sample {age:.1f}s old"
                )
            else:
                line("Station estimate --   Sample age --")
            if status and height >= 24:
                line(
                    f"History {status.history.get('state')} / "
                    f"{status.history.get('interval_seconds')}s   "
                    f"Adapter {status.connection.adapter_id or 'Unknown'}",
                    MUTED,
                )
        content_capacity = height - 4
        # Keep menu within the terminal at small heights; secondary rows yield first.
        rows = rows[: max(1, content_capacity - 3)]
        self.hits = [hit for hit in self.hits if hit.y <= len(rows) + 1]
        while len(rows) < content_capacity - 3:
            line()
        line(self.notice, "yellow" if self.pending else MUTED)
        level = str(status.logging.get("effective_level", "Unknown")) if status else "Unknown"
        menu = [
            ("dashboard", "Dash"),
            ("logs", "Logs"),
            ("retry", "Retry"),
            ("pause", "Resume" if status and status.connection.desired == "paused" else "Pause"),
            ("debug", "Debug:" + ("ON" if level == "DEBUG" else "OFF")),
            ("help", "Help"),
            ("quit", "Quit"),
        ]
        if width < 80:
            short = {
                "dashboard": "D",
                "logs": "L",
                "retry": "R",
                "pause": "P",
                "debug": "B",
                "help": "?",
                "quit": "Q",
            }
            menu = [(action, short[action]) for action, _ in menu]
        buttons(menu)
        self.hits = [hit for hit in self.hits if hit.x2 <= width - 2]
        if self.hits:
            self.focus %= len(self.hits)
        return Panel(
            Group(*rows),
            title=Text("MYPOWERS", style="bold " + BLUE),
            subtitle=Text("Tab focus · Enter activate · q quit", style=MUTED),
            height=height - 1,
            width=width,
            padding=(0, 1),
            border_style=BLUE,
        )
