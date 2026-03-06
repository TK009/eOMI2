"""Section 13 — odf.readItem() E2E device tests.

Exercises readItem through the full OMI write path on a real device:
write values to a target InfoItem, trigger an onwrite script that calls
odf.readItem(), and verify the derived output via OMI read.
"""

import pytest

from helpers import (
    TREE_WRITE_TIMEOUT,
    omi_delete, omi_read, omi_write, omi_write_tree,
    omi_status, omi_result,
)


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /ReadItem and /HVAC after all readItem tests."""
    yield
    for path in ("/ReadItem", "/HVAC"):
        try:
            omi_delete(base_url, path, token=token)
        except Exception:
            pass


def _setup_readitem_tree(base_url, token, items):
    """Create /ReadItem object with the given items dict via tree write."""
    objects = {"ReadItem": {"id": "ReadItem", "items": items}}
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)


def test_readitem_value_suffix(base_url, token):
    """readItem('/path/value') returns the raw primitive, usable in writeItem."""
    _setup_readitem_tree(base_url, token, {
        "Target": {
            "values": [],
            "meta": {"writable": True},
        },
        "Sensor": {
            "values": [],
            "meta": {
                "writable": True,
                "onwrite": "odf.writeItem(odf.readItem('/ReadItem/Target/value'), '/ReadItem/Result');",
            },
        },
        "Result": {
            "values": [],
            "meta": {"writable": True},
        },
    })

    # Write a known value to Target
    data = omi_write(base_url, "/ReadItem/Target", 22.5, token=token)
    assert data["response"]["status"] in (200, 201)

    # Seed Result so we can detect a change
    data = omi_write(base_url, "/ReadItem/Result", -1, token=token)
    assert data["response"]["status"] in (200, 201)

    # Write to Sensor triggers onwrite -> readItem(Target/value) -> writeItem(Result)
    data = omi_write(base_url, "/ReadItem/Sensor", 1, token=token)
    assert data["response"]["status"] in (200, 201)

    # Result should now hold 22.5
    read = omi_read(base_url, "/ReadItem/Result", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 22.5


def test_readitem_element_structure(base_url, token):
    """readItem('/path') (no /value suffix) returns element with .values array."""
    _setup_readitem_tree(base_url, token, {
        "Target": {
            "values": [],
            "meta": {"writable": True},
        },
        "Sensor": {
            "values": [],
            "meta": {
                "writable": True,
                "onwrite": (
                    "let elem = odf.readItem('/ReadItem/Target');"
                    "odf.writeItem(elem.values[0].v, '/ReadItem/Result');"
                ),
            },
        },
        "Result": {
            "values": [],
            "meta": {"writable": True},
        },
    })

    data = omi_write(base_url, "/ReadItem/Target", 42, token=token)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/ReadItem/Result", -1, token=token)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/ReadItem/Sensor", 1, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/ReadItem/Result", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 42


def test_readitem_nonexistent_returns_null(base_url, token):
    """readItem of a nonexistent path returns null (script writes 1 if null)."""
    _setup_readitem_tree(base_url, token, {
        "Sensor": {
            "values": [],
            "meta": {
                "writable": True,
                "onwrite": (
                    "let r = odf.readItem('/ReadItem/NoSuchPath');"
                    "odf.writeItem(r === null ? 1 : 0, '/ReadItem/NullResult');"
                ),
            },
        },
        "NullResult": {
            "values": [],
            "meta": {"writable": True},
        },
    })

    data = omi_write(base_url, "/ReadItem/NullResult", -1, token=token)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/ReadItem/Sensor", 1, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/ReadItem/NullResult", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 1, (
        "readItem of nonexistent path should return null"
    )


def test_readitem_thermostat_scenario(base_url, token):
    """Thermostat: sensor onwrite reads target temp, compares, sets heater."""
    objects = {
        "HVAC": {
            "id": "HVAC",
            "items": {
                "Target": {
                    "values": [],
                    "meta": {"writable": True},
                },
                "Sensor": {
                    "values": [],
                    "meta": {
                        "writable": True,
                        "onwrite": (
                            "let target = odf.readItem('/HVAC/Target/value');"
                            "odf.writeItem(event.value < target, '/HVAC/Heater');"
                        ),
                    },
                },
                "Heater": {
                    "values": [],
                    "meta": {"writable": True},
                },
            },
        },
    }
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # Set target temperature to 22
    data = omi_write(base_url, "/HVAC/Target", 22, token=token)
    assert data["response"]["status"] in (200, 201)

    # Sensor reports 18 (below target) -> heater ON
    data = omi_write(base_url, "/HVAC/Sensor", 18, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/HVAC/Heater", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] is True, (
        "heater should be ON when sensor < target"
    )

    # Sensor reports 25 (above target) -> heater OFF
    data = omi_write(base_url, "/HVAC/Sensor", 25, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/HVAC/Heater", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] is False, (
        "heater should be OFF when sensor > target"
    )


def test_readitem_read_after_write_consistency(base_url, token):
    """Write then readItem in same script sees the just-written value."""
    _setup_readitem_tree(base_url, token, {
        "Shared": {
            "values": [],
            "meta": {"writable": True},
        },
        "Trigger": {
            "values": [],
            "meta": {
                "writable": True,
                "onwrite": (
                    "odf.writeItem(77, '/ReadItem/Shared');"
                    "let r = odf.readItem('/ReadItem/Shared/value');"
                    "odf.writeItem(r, '/ReadItem/RawResult');"
                ),
            },
        },
        "RawResult": {
            "values": [],
            "meta": {"writable": True},
        },
    })

    # Seed Shared with old value
    data = omi_write(base_url, "/ReadItem/Shared", 10, token=token)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/ReadItem/RawResult", -1, token=token)
    assert data["response"]["status"] in (200, 201)

    # Trigger: script writes 77 to Shared, reads it back, writes to RawResult
    data = omi_write(base_url, "/ReadItem/Trigger", 1, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/ReadItem/RawResult", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 77, (
        "read-after-write in same script should see the pending write"
    )


def test_readitem_device_stays_responsive(base_url, token):
    """After readItem tests, device is still responsive."""
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200
