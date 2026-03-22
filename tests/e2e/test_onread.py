"""E2E tests for onread script triggers.

Verifies that onread scripts execute correctly on real hardware:
- HTTP reads of items with onread scripts return computed values
- Interval subscriptions deliver onread-transformed values
- Event subscriptions deliver raw values (no onread)
- Error handling falls back to stored value
- Storage immutability — onread never modifies stored data
- Cascading onread via nested odf.readItem()
- Self-read recursion guard
- Onwrite + onread independence on the same item
"""

import asyncio
import json
import time

import pytest
import websockets

from helpers import (
    TREE_WRITE_TIMEOUT,
    WS_TIMEOUT,
    omi_cancel,
    omi_delete,
    omi_poll,
    omi_read,
    omi_subscribe,
    omi_write,
    omi_write_tree,
    omi_status,
    omi_result,
    run_async,
)

# Main loop ticks every ~5 s; generous timeout for subscription delivery
TICK_WAIT = 12


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove test paths after all onread tests."""
    yield
    for path in ("/OnRead", "/Cascade"):
        try:
            omi_delete(base_url, path, token=token)
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Helper: create an object with an onread item
# ---------------------------------------------------------------------------

def _make_onread_object(name, item_name, script, writable=True, extra_items=None):
    """Build an objects dict for tree write with an onread item."""
    items = {
        item_name: {
            "values": [],
            "meta": {"writable": writable, "onread": script},
        },
    }
    if extra_items:
        items.update(extra_items)
    return {name: {"id": name, "items": items}}


# ---------------------------------------------------------------------------
# Test 1: Basic onread value transformation via HTTP read
# ---------------------------------------------------------------------------


def test_onread_transforms_value(base_url, token):
    """HTTP read of an item with onread returns the script-computed value."""
    objects = _make_onread_object("OnRead", "Double", "event.value * 2")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # Write a raw value
    data = omi_write(base_url, "/OnRead/Double", 21, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read — should get transformed value (21 * 2 = 42)
    read = omi_read(base_url, "/OnRead/Double", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 42


# ---------------------------------------------------------------------------
# Test 2: Storage immutability — onread does not modify stored value
# ---------------------------------------------------------------------------


def test_onread_storage_immutability(base_url, token):
    """Multiple reads return the same transformed value; stored value unchanged."""
    objects = _make_onread_object("OnRead", "Immutable", "event.value + 100")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/Immutable", 5, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read twice — both should return 105, proving stored value (5) is unchanged
    for _ in range(2):
        read = omi_read(base_url, "/OnRead/Immutable", token=token, newest=1)
        assert omi_status(read) == 200
        assert omi_result(read)["values"][0]["v"] == 105


# ---------------------------------------------------------------------------
# Test 3: Onread error fallback — syntax error
# ---------------------------------------------------------------------------


def test_onread_syntax_error_fallback(base_url, token):
    """Broken onread script falls back to returning stored value."""
    objects = _make_onread_object("OnRead", "SyntaxErr", "this is not valid!!!")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/SyntaxErr", 77, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read — should fall back to stored value
    read = omi_read(base_url, "/OnRead/SyntaxErr", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 77

    # Device still responsive
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200


# ---------------------------------------------------------------------------
# Test 4: Onread error fallback — runtime error
# ---------------------------------------------------------------------------


def test_onread_runtime_error_fallback(base_url, token):
    """Runtime error in onread falls back to stored value."""
    objects = _make_onread_object(
        "OnRead", "RuntimeErr", "undefinedVariable.property"
    )
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/RuntimeErr", 33, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/OnRead/RuntimeErr", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 33


# ---------------------------------------------------------------------------
# Test 5: Onread error fallback — infinite loop (op limit)
# ---------------------------------------------------------------------------


def test_onread_infinite_loop_fallback(base_url, token):
    """Infinite-loop onread is terminated; read falls back to stored value."""
    objects = _make_onread_object("OnRead", "Loop", "while(true){} event.value")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/Loop", 88, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read — should fall back to stored value after op limit hit
    read = omi_read(base_url, "/OnRead/Loop", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 88

    # Device still responsive
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200


# ---------------------------------------------------------------------------
# Test 6: Onwrite + onread independence on same item
# ---------------------------------------------------------------------------


def test_onwrite_onread_independence(base_url, token):
    """Item with both onwrite and onread: write triggers onwrite, read triggers onread."""
    items = {
        "Both": {
            "values": [],
            "meta": {
                "writable": True,
                "onwrite": "odf.writeItem(event.value, '/OnRead/Mirror');",
                "onread": "event.value * 10",
            },
        },
    }
    objects = {"OnRead": {"id": "OnRead", "items": items}}
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # Write triggers onwrite cascade
    data = omi_write(base_url, "/OnRead/Both", 7, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read the item — onread transforms: 7 * 10 = 70
    read = omi_read(base_url, "/OnRead/Both", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 70

    # Check onwrite cascade target — should have raw value 7
    mirror = omi_read(base_url, "/OnRead/Mirror", token=token, newest=1)
    assert omi_status(mirror) == 200
    assert omi_result(mirror)["values"][0]["v"] == 7


# ---------------------------------------------------------------------------
# Test 7: Interval subscription delivers onread-transformed values
# ---------------------------------------------------------------------------


def test_interval_sub_with_onread(base_url, token):
    """Interval poll subscription delivers raw values (onread only applies
    to callback/WebSocket deliveries, not poll-buffered values)."""
    objects = _make_onread_object("OnRead", "SubInt", "event.value + 1000")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/SubInt", 5, token=token)
    assert data["response"]["status"] in (200, 201)

    # Create interval poll subscription
    sub = omi_subscribe(base_url, "/OnRead/SubInt", interval=5, ttl=60, token=token)
    assert sub["response"]["status"] == 200
    rid = sub["response"]["rid"]

    try:
        # Wait for tick delivery
        time.sleep(TICK_WAIT)

        # Poll — gets raw buffered value (onread transformation only applies
        # to callback/websocket deliveries, not poll-buffered values)
        poll = omi_poll(base_url, rid, token=token)
        assert poll["response"]["status"] == 200
        values = poll["response"]["result"]["values"]
        assert len(values) > 0, "expected buffered value after interval tick"
        assert values[0]["v"] == 5
    finally:
        omi_cancel(base_url, [rid], token=token)


# ---------------------------------------------------------------------------
# Test 8: Interval subscription over WebSocket with onread
# ---------------------------------------------------------------------------


def test_ws_interval_sub_with_onread(base_url, token, ws_url):
    """WebSocket interval subscription delivers onread-transformed values."""
    objects = _make_onread_object("OnRead", "WsInt", "event.value * 3")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/WsInt", 11, token=token)
    assert data["response"]["status"] in (200, 201)

    async def _test():
        async with websockets.connect(ws_url, open_timeout=WS_TIMEOUT) as ws:
            sub_msg = json.dumps({
                "omi": "1.0",
                "ttl": 60,
                "read": {"path": "/OnRead/WsInt", "interval": 5},
            })
            await ws.send(sub_msg)
            resp_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
            resp = json.loads(resp_raw)
            assert resp["response"]["status"] == 200
            rid = resp["response"]["rid"]

            try:
                # Wait for interval push
                push_raw = await asyncio.wait_for(ws.recv(), timeout=TICK_WAIT)
                push = json.loads(push_raw)
                assert push["response"]["status"] == 200
                assert push["response"]["rid"] == rid
                # Verify transformed value: 11 * 3 = 33
                values = push["response"]["result"]["values"]
                assert len(values) > 0
                assert values[0]["v"] == 33
            finally:
                cancel_msg = json.dumps({
                    "omi": "1.0",
                    "ttl": 10,
                    "cancel": {"rid": [rid]},
                })
                await ws.send(cancel_msg)
                await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)

    run_async(_test())


# ---------------------------------------------------------------------------
# Test 9: Event subscription delivers raw value (no onread)
# ---------------------------------------------------------------------------


def test_event_sub_no_onread(base_url, token):
    """Event subscription (interval=-1) delivers raw written value, not onread-transformed."""
    objects = _make_onread_object("OnRead", "EvtRaw", "event.value * 999")
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/EvtRaw", 10, token=token)
    assert data["response"]["status"] in (200, 201)

    # Create event poll subscription
    sub = omi_subscribe(base_url, "/OnRead/EvtRaw", interval=-1, ttl=60, token=token)
    assert sub["response"]["status"] == 200
    rid = sub["response"]["rid"]

    try:
        # Drain any initial buffer
        omi_poll(base_url, rid, token=token)

        # Write new value — triggers event notification
        omi_write(base_url, "/OnRead/EvtRaw", 20, token=token)

        # Poll — should get raw value 20, NOT 20 * 999
        poll = omi_poll(base_url, rid, token=token)
        assert poll["response"]["status"] == 200
        values = poll["response"]["result"]["values"]
        assert len(values) > 0, "expected event sub to buffer value on write"
        assert values[0]["v"] == 20, (
            f"event sub should deliver raw value, got {values[0]['v']}"
        )
    finally:
        omi_cancel(base_url, [rid], token=token)


# ---------------------------------------------------------------------------
# Test 10: Cascading onread via odf.readItem()
# ---------------------------------------------------------------------------


def test_cascading_onread(base_url, token):
    """Onread script that calls odf.readItem() triggers nested onread."""
    items = {
        "Inner": {
            "values": [],
            "meta": {"writable": True, "onread": "event.value + 100"},
        },
        "Outer": {
            "values": [],
            "meta": {
                "writable": True,
                "onread": "odf.readItem('/Cascade/Inner/value') * 2",
            },
        },
    }
    objects = {"Cascade": {"id": "Cascade", "items": items}}
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    # Write raw values
    data = omi_write(base_url, "/Cascade/Inner", 5, token=token)
    assert data["response"]["status"] in (200, 201)
    data = omi_write(base_url, "/Cascade/Outer", 0, token=token)
    assert data["response"]["status"] in (200, 201)

    # Read Inner directly — should get 5 + 100 = 105
    read = omi_read(base_url, "/Cascade/Inner", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 105

    # Read Outer — script calls readItem("/Cascade/Inner") which triggers Inner's
    # onread (5 + 100 = 105), then Outer returns 105 * 2 = 210
    read = omi_read(base_url, "/Cascade/Outer", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 210


# ---------------------------------------------------------------------------
# Test 11: Self-read recursion guard
# ---------------------------------------------------------------------------


def test_self_read_recursion_guard(base_url, token):
    """Onread that reads its own path gets stored value (no infinite loop)."""
    objects = _make_onread_object(
        "OnRead", "SelfRead", "odf.readItem('/OnRead/SelfRead') + 1"
    )
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/SelfRead", 50, token=token)
    assert data["response"]["status"] in (200, 201)

    # Self-read returns stored value (50), so script returns 50 + 1 = 51
    # If recursion guard fails, this would infinite-loop or hit op limit
    read = omi_read(base_url, "/OnRead/SelfRead", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == 51

    # Device still responsive
    check = omi_read(base_url, "/")
    assert omi_status(check) == 200


# ---------------------------------------------------------------------------
# Test 12: String transformation in onread
# ---------------------------------------------------------------------------


def test_onread_string_transform(base_url, token):
    """Onread can transform string values."""
    objects = _make_onread_object(
        "OnRead", "StrXform", "String(event.value).toUpperCase()"
    )
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201)

    data = omi_write(base_url, "/OnRead/StrXform", "hello", token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/OnRead/StrXform", token=token, newest=1)
    assert omi_status(read) == 200
    assert omi_result(read)["values"][0]["v"] == "HELLO"
