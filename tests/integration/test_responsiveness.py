"""A full scan deadline must not hold HTTP health, history or diagnostic requests."""

import asyncio
import json
import os
import threading
import time
from pathlib import Path

from fastapi.testclient import TestClient

from mypowers.api import create_app
from mypowers.contracts import AppError
from mypowers.daemon.service import Service

RESULT_FILE = os.environ.get("MYPOWERS_API_LOAD_RESULT_FILE")


def test_status_remains_responsive_through_twenty_second_scan(config):
    started, finished = threading.Event(), threading.Event()

    class Scanning:
        adapter_id = "fake-scan"

        async def acquire(self, phase):
            phase("scanning")
            started.set()
            await asyncio.sleep(20)
            finished.set()
            raise AppError("station_not_found", "Simulated scan exhausted.", 503)

        async def cleanup(self):
            pass

    latency = []
    service = Service(config, Scanning)
    with TestClient(create_app(config, service)) as client:
        assert started.wait(2)
        before = time.monotonic()
        while not finished.is_set():
            request_start = time.monotonic()
            response = client.get("/api/v1/status")
            latency.append(time.monotonic() - request_start)
            assert response.status_code == 200
            data = response.json()
            assert data["telemetry"]["sample"] is None and not data["controls"]["allowed"]
            assert client.get("/health/live").status_code == 200
            assert client.get("/api/v1/history").status_code in (200, 503)
            assert client.get("/api/v1/logs?limit=10").status_code == 200
            time.sleep(0.5)
        elapsed = time.monotonic() - before
        assert elapsed >= 19.5 and len(latency) >= 30
        p95 = sorted(latency)[int(len(latency) * 0.95)]
        assert p95 < 0.5
        if RESULT_FILE:
            Path(RESULT_FILE).write_text(
                json.dumps(
                    {
                        "result": "PASS",
                        "transport": "fake twenty-second scan then station_not_found",
                        "elapsed_seconds": elapsed,
                        "requests": len(latency),
                        "status_p95_ms": p95 * 1000,
                        "target_p95_ms": 500,
                    },
                    indent=2,
                )
                + "\n"
            )
