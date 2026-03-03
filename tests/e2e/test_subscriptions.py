"""Section 7 — Subscription E2E Tests.

Tests for poll subscriptions, event subscriptions over WebSocket,
interval subscriptions, TTL expiry, and cancellation.
"""

import asyncio
import json
import time

import pytest
import websockets

from helpers import (
    WS_TIMEOUT,
    omi_cancel,
    omi_delete,
    omi_poll,
    omi_subscribe,
    omi_write,
    run_async,
)


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /Test after all subscription tests."""
    yield
    try:
        omi_delete(base_url, "/Test", token=token)
    except Exception:
        pass


# ── Test 1: Poll subscription lifecycle ──────────────────────────────────


def test_poll_sub_lifecycle(base_url, token):
    """Create poll sub, write value, poll for it, poll again for empty."""
    # Create event-based poll subscription on /Test/SubVal
    data = omi_subscribe(base_url, "/Test/SubVal", interval=-1, ttl=60, token=token)
    assert data["response"]["status"] == 200
    rid = data["response"]["rid"]
    assert rid

    try:
        # Write a value to trigger the subscription
        omi_write(base_url, "/Test/SubVal", 42, token=token)

        # Poll — expect the written value
        poll = omi_poll(base_url, rid, token=token)
        assert poll["response"]["status"] == 200
        values = poll["response"]["result"]["values"]
        assert len(values) > 0, "expected at least one buffered value"
        assert any(v["v"] == 42 for v in values)

        # Poll again — buffer should be drained
        poll2 = omi_poll(base_url, rid, token=token)
        assert poll2["response"]["status"] == 200
        values2 = poll2["response"]["result"]["values"]
        assert len(values2) == 0, "expected empty buffer after drain"
    finally:
        omi_cancel(base_url, [rid], token=token)


# ── Test 2: Poll subscription TTL expiry ─────────────────────────────────


def test_poll_sub_expiry(base_url, token):
    """Poll sub with short TTL expires; polling afterwards returns 404."""
    data = omi_subscribe(base_url, "/Test/SubExp", interval=-1, ttl=3, token=token)
    assert data["response"]["status"] == 200
    rid = data["response"]["rid"]

    # Wait for expiry (3x the TTL for generous margin)
    time.sleep(9)

    # Poll expired subscription — expect 404
    poll = omi_poll(base_url, rid, token=token, check=False)
    assert poll["response"]["status"] == 404


# ── Test 3: Event subscription over WebSocket ───────────────────────────


def test_event_sub_on_ws(base_url, token, ws_url):
    """Subscribe over WS, write a value via HTTP, receive push on WS."""

    async def _test():
        async with websockets.connect(ws_url, open_timeout=WS_TIMEOUT) as ws:
            # Send subscription request over WS
            sub_msg = json.dumps({
                "omi": "1.0",
                "ttl": 60,
                "read": {"path": "/Test/SubEvt", "interval": -1},
            })
            await ws.send(sub_msg)
            resp_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
            resp = json.loads(resp_raw)
            assert resp["response"]["status"] == 200
            rid = resp["response"]["rid"]
            assert rid

            try:
                # Write value via HTTP to trigger the event
                omi_write(base_url, "/Test/SubEvt", "ws-event", token=token)

                # Expect a push message on the WS
                push_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
                push = json.loads(push_raw)
                assert push["response"]["status"] == 200
                assert push["response"]["rid"] == rid
                values = push["response"]["result"]["values"]
                assert len(values) > 0
                assert any(v["v"] == "ws-event" for v in values)
            finally:
                # Cancel via WS
                cancel_msg = json.dumps({
                    "omi": "1.0",
                    "ttl": 10,
                    "cancel": {"rid": [rid]},
                })
                await ws.send(cancel_msg)
                cancel_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
                cancel_resp = json.loads(cancel_raw)
                assert cancel_resp["response"]["status"] == 200

    run_async(_test())


# ── Test 4: Interval subscription ────────────────────────────────────────


def test_interval_sub(base_url, token, ws_url):
    """Interval subscription delivers at least one push within the interval."""
    # Write an initial value so the path exists for interval reads
    omi_write(base_url, "/Test/SubInterval", "initial", token=token)

    async def _test():
        async with websockets.connect(ws_url, open_timeout=WS_TIMEOUT) as ws:
            # Subscribe with 3 s interval
            sub_msg = json.dumps({
                "omi": "1.0",
                "ttl": 30,
                "read": {"path": "/Test/SubInterval", "interval": 3},
            })
            await ws.send(sub_msg)
            resp_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
            resp = json.loads(resp_raw)
            assert resp["response"]["status"] == 200
            rid = resp["response"]["rid"]

            try:
                # Wait for at least one push (generous timeout)
                push_raw = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
                push = json.loads(push_raw)
                assert push["response"]["status"] == 200
                assert push["response"]["rid"] == rid
                assert "result" in push["response"]
            finally:
                cancel_msg = json.dumps({
                    "omi": "1.0",
                    "ttl": 10,
                    "cancel": {"rid": [rid]},
                })
                await ws.send(cancel_msg)
                await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)

    run_async(_test())


# ── Test 5: Cancel subscription ──────────────────────────────────────────


def test_cancel_sub(base_url, token):
    """Cancelling a poll sub makes subsequent polls return 404."""
    data = omi_subscribe(
        base_url, "/Test/SubCancel", interval=-1, ttl=60, token=token
    )
    assert data["response"]["status"] == 200
    rid = data["response"]["rid"]

    # Cancel immediately
    cancel = omi_cancel(base_url, [rid], token=token)
    assert cancel["response"]["status"] == 200

    # Write a value (should not be buffered)
    omi_write(base_url, "/Test/SubCancel", "nope", token=token)

    # Poll — subscription is gone
    poll = omi_poll(base_url, rid, token=token, check=False)
    assert poll["response"]["status"] == 404
