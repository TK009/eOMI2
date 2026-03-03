"""Section 2 — HTTP Read Operations.

Verify OMI read functionality over POST /omi.  The device always returns
HTTP 200 for POST /omi; the OMI-level status (200, 404, …) lives inside
the JSON response body at ``response.status``.
"""

import time

import requests

from helpers import omi_read


# -- helpers -----------------------------------------------------------------

def _status(data):
    """Extract the OMI-level status code from a response envelope."""
    return data["response"]["status"]


def _result(data):
    """Extract the result payload from a response envelope."""
    return data["response"]["result"]


# -- tests -------------------------------------------------------------------

def test_read_root(base_url):
    """Reading '/' returns the object tree with a System entry."""
    data = omi_read(base_url, path="/")
    assert _status(data) == 200
    result = _result(data)
    assert "System" in result


def test_read_sensor_object(base_url):
    """Reading '/System' lists the FreeHeap item."""
    data = omi_read(base_url, path="/System")
    assert _status(data) == 200
    result = _result(data)
    items = result["items"]
    assert "FreeHeap" in items


def test_read_sensor_value(base_url):
    """Reading a sensor value path returns at least one measurement."""
    # The main loop writes free-heap every 5 s; poll a few times to allow
    # for the first iteration after boot.
    values = []
    for _ in range(6):
        time.sleep(5)
        try:
            data = omi_read(base_url, path="/System/FreeHeap")
        except requests.RequestException:
            continue
        assert _status(data) == 200
        result = _result(data)
        values = result["values"]
        if len(values) >= 1:
            break
    assert len(values) >= 1, "Sensor produced no readings after 30 s"


def test_read_newest(base_url):
    """Reading with newest=1 returns at most one value."""
    data = omi_read(base_url, path="/System/FreeHeap", newest=1)
    assert _status(data) == 200
    result = _result(data)
    assert len(result["values"]) <= 1


def test_read_nonexistent(base_url):
    """Reading a path that does not exist returns OMI status 404."""
    data = omi_read(base_url, path="/NoSuch")
    assert _status(data) == 404


def test_read_with_depth(base_url):
    """Reading '/System' with depth=0 returns System id but no nested items."""
    data = omi_read(base_url, path="/System", depth=0)
    assert _status(data) == 200
    result = _result(data)
    assert result["id"] == "System"
    # depth=0 should omit nested items entirely
    assert "items" not in result or result["items"] == {}
