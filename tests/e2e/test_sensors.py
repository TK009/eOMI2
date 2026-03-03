"""Section 11 — Sensor Readings.

Verify that the device produces sensor values that are numeric,
within a plausible range, and update over time.

The current firmware exposes System/FreeHeap (bytes of free heap
memory) as its only sensor.  The main loop writes a new reading
every 5 seconds.
"""

import time

import pytest

from helpers import omi_read

SENSOR_PATH = "/System/FreeHeap"
# Back-off delays while waiting for sensor values to appear
_BACKOFF = [1, 2, 3, 5, 5, 5]


def _status(data):
    return data["response"]["status"]

def _result(data):
    return data["response"]["result"]

def _wait_for_values(base_url, min_count=1):
    """Poll sensor path until at least *min_count* values exist."""
    for delay in _BACKOFF:
        time.sleep(delay)
        data = omi_read(base_url, path=SENSOR_PATH)
        if _status(data) == 200 and len(_result(data)["values"]) >= min_count:
            return _result(data)["values"]
    total = sum(_BACKOFF)
    pytest.fail(f"No values at {SENSOR_PATH} after {total}s")


def test_free_heap_in_range(base_url):
    """FreeHeap value is a number between 1 KB and 16 MB."""
    values = _wait_for_values(base_url)
    v = values[0]["v"]
    assert isinstance(v, (int, float)), f"expected number, got {type(v)}"
    assert 1_000 <= v <= 16_000_000, f"FreeHeap {v} out of plausible range"


def test_values_update(base_url):
    """Two readings taken ≥6s apart have different timestamps."""
    _wait_for_values(base_url)
    data1 = omi_read(base_url, path=SENSOR_PATH, newest=1)
    t1 = _result(data1)["values"][0]["t"]

    time.sleep(6)  # main loop writes every 5s

    data2 = omi_read(base_url, path=SENSOR_PATH, newest=1)
    t2 = _result(data2)["values"][0]["t"]
    assert t2 != t1, f"Timestamp did not change: {t1}"
