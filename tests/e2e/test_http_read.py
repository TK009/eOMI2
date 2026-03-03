"""Section 2 — HTTP Read Operations.

Verify OMI read functionality over POST /omi.  The device always returns
HTTP 200 for POST /omi; the OMI-level status (200, 404, …) lives inside
the JSON response body at ``response.status``.
"""

from helpers import omi_read, omi_status, omi_result, wait_for_values


# -- tests -------------------------------------------------------------------

def test_read_root(base_url):
    """Reading '/' returns the object tree with a System entry."""
    data = omi_read(base_url, path="/")
    assert omi_status(data) == 200
    result = omi_result(data)
    assert "System" in result


def test_read_sensor_object(base_url):
    """Reading '/System' lists the FreeHeap item."""
    data = omi_read(base_url, path="/System")
    assert omi_status(data) == 200
    result = omi_result(data)
    items = result["items"]
    assert "FreeHeap" in items


def test_read_sensor_value(base_url):
    """Reading a sensor value path returns at least one measurement."""
    values = wait_for_values(base_url)
    assert len(values) >= 1


def test_read_newest(base_url):
    """Reading with newest=1 returns exactly one value."""
    wait_for_values(base_url)
    data = omi_read(base_url, path="/System/FreeHeap", newest=1)
    assert omi_status(data) == 200
    result = omi_result(data)
    assert len(result["values"]) == 1


def test_read_nonexistent(base_url):
    """Reading a path that does not exist returns OMI status 404."""
    data = omi_read(base_url, path="/NoSuch")
    assert omi_status(data) == 404


def test_read_with_depth(base_url):
    """Reading '/System' with depth=0 returns System id but no nested items."""
    data = omi_read(base_url, path="/System", depth=0)
    assert omi_status(data) == 200
    result = omi_result(data)
    assert result["id"] == "System"
    # depth=0 should omit nested items entirely
    assert "items" not in result or result["items"] == {}
