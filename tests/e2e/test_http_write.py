"""Section 3 — HTTP Write Operations.

Tests for single writes (various types), overwrite semantics, sensor
protection, batch writes, and tree-merge writes.
"""

import pytest

from helpers import omi_delete, omi_read, omi_write, omi_write_batch, omi_write_tree


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /Test (and /TestTree if present) after all write tests."""
    yield
    for path in ("/Test", "/TestTree"):
        try:
            omi_delete(base_url, path, token=token)
        except Exception:
            pass


def test_write_new_item(base_url, token):
    """Write a new numeric value and read it back."""
    data = omi_write(base_url, "/Test/Value", 42, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/Test/Value", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] == 42


def test_write_string_value(base_url, token):
    """Write a string value and read it back."""
    data = omi_write(base_url, "/Test/Str", "hello", token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/Test/Str", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] == "hello"


def test_write_bool_value(base_url, token):
    """Write a boolean value and read it back."""
    data = omi_write(base_url, "/Test/Bool", True, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/Test/Bool", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] is True


def test_write_overwrite(base_url, token):
    """Writing twice overwrites; newest value is the last one written."""
    omi_write(base_url, "/Test/Over", 1, token=token)
    omi_write(base_url, "/Test/Over", 2, token=token)

    read = omi_read(base_url, "/Test/Over", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] == 2


def test_write_sensor_rejected(base_url, token):
    """Writing to a hardware-owned sensor path is rejected (403)."""
    data = omi_write(base_url, "/Dht11/Temperature", 99, token=token)
    assert data["response"]["status"] == 403
    assert data["response"].get("desc")


def test_write_batch(base_url, token):
    """Batch write multiple items and verify each."""
    items = [
        {"path": "/Test/BatchA", "v": 10},
        {"path": "/Test/BatchB", "v": "bee"},
        {"path": "/Test/BatchC", "v": True},
    ]
    data = omi_write_batch(base_url, items, token=token)
    assert data["response"]["status"] == 200

    results = data["response"]["result"]
    assert isinstance(results, list), f"expected list, got {type(results)}"
    for r in results:
        assert r["status"] in (200, 201), f"{r['path']} failed: {r}"

    # Read each back
    for item in items:
        read = omi_read(base_url, item["path"], token=token, newest=1)
        assert read["response"]["status"] == 200
        values = read["response"]["result"]["values"]
        assert values[0]["v"] == item["v"]


@pytest.mark.xfail(reason="tree write crashes device — firmware bug to investigate")
def test_write_tree_merge(base_url, token):
    """Tree write merges an object subtree; verify with a single write + read."""
    # Step 1: tree write creates the object hierarchy (no items/values — just structure)
    objects = {
        "TestTree": {
            "id": "TestTree",
            "objects": {
                "Sub": {"id": "Sub"},
            },
        },
    }
    # Extended timeout — tree writes are slow and this test is xfail due to firmware crash
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=30)
    assert data["response"]["status"] in (200, 201)

    # Step 2: single write into the new subtree to prove it exists
    data = omi_write(base_url, "/TestTree/Sub/Leaf", 99, token=token)
    assert data["response"]["status"] in (200, 201)

    # Step 3: read back the value
    read = omi_read(base_url, "/TestTree/Sub/Leaf", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] == 99
