"""Section 11 — Sensor Readings.

Verify that the device produces sensor values that are numeric,
within a plausible range, and update over time.

The current firmware exposes System/FreeHeap (bytes of free heap
memory) as its only sensor.  The main loop writes a new reading
every 5 seconds.
"""

import time

import pytest
import requests

from helpers import POLL_BACKOFF, omi_read, omi_result, wait_for_values

SENSOR_PATH = "/System/FreeHeap"


def test_free_heap_in_range(base_url):
    """FreeHeap value is a number between 1 KB and 16 MB."""
    values = wait_for_values(base_url)
    v = values[0]["v"]
    assert isinstance(v, (int, float)), f"expected number, got {type(v)}"
    assert 1_000 <= v <= 16_000_000, f"FreeHeap {v} out of plausible range"


def test_values_update(base_url):
    """A newer reading has a strictly later timestamp."""
    wait_for_values(base_url)
    data1 = omi_read(base_url, path=SENSOR_PATH, newest=1)
    t1 = omi_result(data1)["values"][0]["t"]

    for delay in POLL_BACKOFF:
        time.sleep(delay)
        try:
            data2 = omi_read(base_url, path=SENSOR_PATH, newest=1)
        except requests.RequestException:
            continue
        t2 = omi_result(data2)["values"][0]["t"]
        if t2 > t1:
            return
    total = sum(POLL_BACKOFF)
    pytest.fail(f"Timestamp did not advance beyond {t1} after {total}s")
