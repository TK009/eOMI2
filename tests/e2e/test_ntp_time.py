"""E2E test: verify wall-clock timestamps after SNTP sync.

Write a value to the O-DF tree, read it back, and assert the timestamp
is plausible wall-clock time (> 2025-01-01, i.e. > 1735689600 epoch
seconds — not 0 or near-zero).  Also verify timestamps survive a
read→write→read cycle.
"""

import time

import pytest

from helpers import (
    omi_delete,
    omi_read,
    omi_result,
    omi_status,
    omi_write,
    wait_for_values,
)

# 2025-01-01T00:00:00 UTC
WALL_CLOCK_FLOOR = 1_735_689_600

TEST_PATH = "/Test/NtpTime"


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_path(base_url, token):
    """Remove /Test/NtpTime after all tests in this module."""
    yield
    try:
        omi_delete(base_url, TEST_PATH, token=token)
    except Exception:
        pass


def _assert_wall_clock(timestamp, label="timestamp"):
    """Assert *timestamp* looks like a real wall-clock value."""
    assert isinstance(timestamp, (int, float)), (
        f"{label}: expected number, got {type(timestamp)}"
    )
    assert timestamp > WALL_CLOCK_FLOOR, (
        f"{label}: {timestamp} is not wall-clock time "
        f"(expected > {WALL_CLOCK_FLOOR})"
    )


# -- tests -------------------------------------------------------------------


def _wait_for_ntp_sync(base_url, timeout=60):
    """Poll FreeHeap until timestamps reflect wall-clock time (NTP synced)."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            values = wait_for_values(base_url, path="/System/FreeHeap")
            if values and values[0].get("t", 0) > WALL_CLOCK_FLOOR:
                return values
        except Exception:
            pass
        time.sleep(5)
    pytest.fail(f"NTP did not sync within {timeout}s")


def test_sensor_timestamp_is_wall_clock(base_url):
    """A system sensor reading has a wall-clock timestamp (> 2025-01-01)."""
    values = _wait_for_ntp_sync(base_url, timeout=60)
    _assert_wall_clock(values[0]["t"], "FreeHeap timestamp")


def test_written_value_has_wall_clock_timestamp(base_url, token):
    """Write a value, read it back, assert its timestamp is wall-clock."""
    resp = omi_write(base_url, TEST_PATH, 123, token=token)
    assert resp["response"]["status"] in (200, 201)

    read = omi_read(base_url, TEST_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    _assert_wall_clock(values[0]["t"], "written value timestamp")


def test_timestamp_survives_read_write_read(base_url, token):
    """Timestamps survive a read→write→read cycle and remain wall-clock."""
    # First write
    omi_write(base_url, TEST_PATH, "alpha", token=token)

    # Read 1
    read1 = omi_read(base_url, TEST_PATH, token=token, newest=1)
    assert omi_status(read1) == 200
    t1 = omi_result(read1)["values"][0]["t"]
    _assert_wall_clock(t1, "read-1 timestamp")

    # Small delay so the second write gets a different timestamp
    time.sleep(1)

    # Second write (overwrite)
    omi_write(base_url, TEST_PATH, "beta", token=token)

    # Read 2
    read2 = omi_read(base_url, TEST_PATH, token=token, newest=1)
    assert omi_status(read2) == 200
    v2 = omi_result(read2)["values"][0]
    t2 = v2["t"]
    _assert_wall_clock(t2, "read-2 timestamp")

    # Value should be the latest write
    assert v2["v"] == "beta"

    # Second timestamp should be >= first (time moves forward)
    assert t2 >= t1, (
        f"timestamp did not advance: t1={t1}, t2={t2}"
    )
