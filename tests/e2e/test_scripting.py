"""Section 12 — Scripting.

Tests for onwrite script execution triggered by value writes over the network.
"""

import pytest

from helpers import (
    TREE_WRITE_TIMEOUT,
    omi_delete, omi_read, omi_write, omi_write_tree,
    omi_status, omi_result,
)


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /Script and /Chain after all scripting tests."""
    yield
    for path in ("/Script", "/Chain"):
        try:
            omi_delete(base_url, path, token=token)
        except Exception:
            pass


def test_onwrite_cascade(base_url, token):
    """Write item with onwrite script that copies value to another path."""
    # 1. Tree write: create /Script object with Src item (has onwrite).
    #    "values": [] is required because InfoItem.values (RingBuffer) is
    #    non-optional and has no serde default.
    objects = {
        "Script": {
            "id": "Script",
            "items": {
                "Src": {
                    "values": [],
                    "meta": {
                        "writable": True,
                        "onwrite": "odf.writeItem(event.value, '/Script/Dst');",
                    },
                },
            },
        },
    }
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # 2. Single write to Src → triggers onwrite → cascades to /Script/Dst
    data = omi_write(base_url, "/Script/Src", 42, token=token)
    assert data["response"]["status"] in (200, 201)

    # 3. Read /Script/Dst — should have the cascaded value
    read = omi_read(base_url, "/Script/Dst", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 42


def test_script_error_no_crash(base_url, token):
    """Broken onwrite script does not block the write or crash the device."""
    # 1. Tree write: create item with broken script
    objects = {
        "Script": {
            "id": "Script",
            "items": {
                "Bad": {
                    "values": [],
                    "meta": {
                        "writable": True,
                        "onwrite": "this is not valid javascript!!!",
                    },
                },
            },
        },
    }
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # 2. Write — should succeed despite broken script
    data = omi_write(base_url, "/Script/Bad", 99, token=token)
    assert data["response"]["status"] in (200, 201)

    # 3. Value was written
    read = omi_read(base_url, "/Script/Bad", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 99

    # 4. Device still responsive
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200


def test_infinite_loop_device_stays_responsive(base_url, token):
    """Infinite-loop onwrite script is terminated; device remains responsive."""
    # 1. Tree write: create item with while(true){} script
    objects = {
        "Script": {
            "id": "Script",
            "items": {
                "Loop": {
                    "values": [],
                    "meta": {
                        "writable": True,
                        "onwrite": "while(true){}",
                    },
                },
            },
        },
    }
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # 2. Write to trigger the infinite-loop script — should return 200 with
    #    a warning desc (partial-success: write OK, script failed)
    data = omi_write(base_url, "/Script/Loop", 55, token=token)
    assert data["response"]["status"] == 200
    desc = data["response"].get("desc")
    assert desc is not None, "expected warning desc for op-limit script"
    assert "operation limit" in desc or "time limit" in desc, (
        f"desc should mention limit, got: {desc}"
    )

    # 3. Value was written despite script failure
    read = omi_read(base_url, "/Script/Loop", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 55

    # 4. Subsequent writes still work normally (device not wedged)
    data = omi_write(base_url, "/Script/Loop", 66, token=token)
    assert data["response"]["status"] == 200

    read = omi_read(base_url, "/Script/Loop", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 66

    # 5. Device is responsive for unrelated operations
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200


def test_cascade_depth_limit(base_url, token):
    """Deep cascade chain is capped by MAX_SCRIPT_DEPTH; device stays alive."""
    # Build a chain: /Chain/L0 → L1 → ... → L6
    # MAX_SCRIPT_DEPTH is 4 on the device, so L4+ should NOT be updated.
    chain_len = 7
    items = {}
    for i in range(chain_len):
        item = {"values": [], "meta": {"writable": True}}
        if i < chain_len - 1:
            item["meta"]["onwrite"] = (
                f"odf.writeItem(event.value, '/Chain/L{i + 1}');"
            )
        items[f"L{i}"] = item

    objects = {"Chain": {"id": "Chain", "items": items}}
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # Seed all items with -1 so we can distinguish updated vs. not
    for i in range(chain_len):
        data = omi_write(base_url, f"/Chain/L{i}", -1, token=token)
        assert data["response"]["status"] in (200, 201)

    # Trigger the chain
    data = omi_write(base_url, "/Chain/L0", 77, token=token)
    assert data["response"]["status"] in (200, 201)

    # Items within depth limit (L0..L3) should be 77
    for i in range(4):
        read = omi_read(base_url, f"/Chain/L{i}", token=token, newest=1)
        assert omi_status(read) == 200
        assert omi_result(read)["values"][0]["v"] == 77, (
            f"/Chain/L{i} should have been updated"
        )

    # Items beyond depth limit (L4+) should still be -1
    for i in range(4, chain_len):
        read = omi_read(base_url, f"/Chain/L{i}", token=token, newest=1)
        assert omi_status(read) == 200
        assert omi_result(read)["values"][0]["v"] == -1, (
            f"/Chain/L{i} should NOT have been updated"
        )

    # Device still responsive
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200
