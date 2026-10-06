#!/usr/bin/env python3
"""Capture read-only daemon data for the production UI snapshot renderer."""

import argparse
import asyncio
import json
from datetime import datetime, timedelta
from pathlib import Path

from mypowers.client import Client
from mypowers.config import client_config


async def capture(output: Path) -> None:
    async with Client(client_config()) as client:
        status = await client.request("GET", "/status")
        settings = await client.request("GET", "/settings")
        now = datetime.fromisoformat(status["server_time"].replace("Z", "+00:00"))
        history = {}
        for seconds in (10, 30, 60, 3600):
            history[str(seconds)] = await client.request(
                "GET",
                "/history/aggregates",
                params={
                    "since": (now - timedelta(seconds=seconds * 255)).isoformat(),
                    "until": (now + timedelta(milliseconds=1)).isoformat(),
                    "bucket_seconds": seconds,
                    "limit": 256,
                },
            )
        logs = await client.request("GET", "/logs", params={"tail": 100, "min_level": "DEBUG"})
        logs_oldest = await client.request(
            "GET", "/logs", params={"direction": "forward", "min_level": "DEBUG", "limit": 1}
        )
    await asyncio.to_thread(output.parent.mkdir, parents=True, exist_ok=True)
    await asyncio.to_thread(
        output.write_text,
        json.dumps(
            {
                "status": status,
                "settings": settings,
                "history": history,
                "logs": logs,
                "logs_oldest": logs_oldest,
            },
            indent=2,
        )
        + "\n",
    )
    print(f"Captured telemetry and database history at {status['server_time']} to {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    asyncio.run(capture(parser.parse_args().output))
