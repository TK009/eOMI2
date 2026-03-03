"""Section 12 — Scripting.

Tests for onwrite script execution triggered by value writes over the network.
"""

import pytest

from helpers import (
    omi_delete, omi_read, omi_write, omi_write_tree,
    omi_status, omi_result,
)


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /Script after all scripting tests."""
    yield
    for path in ("/Script",):
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
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=30)
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
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=30)
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
