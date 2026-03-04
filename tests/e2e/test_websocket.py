"""WebSocket e2e tests — reachability, reads, subscriptions, and concurrency.

Verifies that /omi/ws is reachable even though the GET /omi/* wildcard is
also registered.  This catches handler-registration-order regressions
(ESP_ERR_HTTPD_HANDLER_EXISTS).

Subscription tests use interval-based subscriptions (interval>0) which fire
via tick() in the main loop (~5s between cycles).  Event-based subscriptions
(interval=-1) are also supported and fire immediately on writes.
"""

import asyncio
import json

import pytest
import websockets

from helpers import WS_TIMEOUT, run_async

SUB_PUSH_TIMEOUT = 20  # seconds — wait for interval sub delivery (~2-4 ticks)
SUB_PATH = "/System/FreeHeap"  # coupled to device tree — update if sensors change
SUB_INTERVAL = 5  # seconds — matches main loop tick period
SUB_TTL = 60


# ---------------------------------------------------------------------------
# Async helpers
# ---------------------------------------------------------------------------

async def _ws_read(ws_url, path="/"):
    """Open a WS connection, send a one-time OMI read, return parsed response."""
    async with websockets.connect(ws_url, open_timeout=WS_TIMEOUT) as ws:
        msg = json.dumps({"omi": "1.0", "ttl": 0, "read": {"path": path}})
        await ws.send(msg)
        resp = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
        return json.loads(resp)


async def _ws_subscribe(ws_url, path=SUB_PATH, interval=SUB_INTERVAL, ttl=SUB_TTL):
    """Connect, create an interval subscription, return (ws, rid)."""
    ws = await websockets.connect(ws_url, open_timeout=WS_TIMEOUT)
    try:
        msg = json.dumps({
            "omi": "1.0",
            "ttl": ttl,
            "read": {"path": path, "interval": interval},
        })
        await ws.send(msg)
        resp = json.loads(await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT))
        assert resp["response"]["status"] == 200, f"subscribe failed: {resp}"
        rid = resp["response"]["rid"]
        assert rid, "subscription response missing rid"
        return ws, rid
    except BaseException:
        await ws.close()
        raise


async def _ws_cancel(ws, rids):
    """Send a cancel request for the given rid list on an open WS."""
    msg = json.dumps({"omi": "1.0", "ttl": 0, "cancel": {"rid": rids}})
    await ws.send(msg)
    resp = json.loads(await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT))
    return resp


# ---------------------------------------------------------------------------
# Basic tests
# ---------------------------------------------------------------------------

def test_ws_endpoint_reachable(ws_url):
    """WebSocket upgrade to /omi/ws succeeds (not claimed by GET /omi/*)."""
    data = run_async(_ws_read(ws_url))
    assert data["omi"] == "1.0"
    assert data["response"]["status"] == 200


def test_ws_read_root(ws_url):
    """OMI read over WS returns the object tree root."""
    data = run_async(_ws_read(ws_url, path="/"))
    result = data["response"]["result"]
    assert isinstance(result, (list, dict))
    assert len(result) > 0, "OMI root tree is empty"


# ---------------------------------------------------------------------------
# Subscription tests
# ---------------------------------------------------------------------------

def test_ws_event_sub(ws_url):
    """Interval subscription delivers push messages over WS."""

    async def _test():
        ws, rid = await _ws_subscribe(ws_url)
        try:
            # Wait for a push delivery
            push = json.loads(
                await asyncio.wait_for(ws.recv(), timeout=SUB_PUSH_TIMEOUT)
            )
            assert push["response"]["status"] == 200
            assert push["response"]["rid"] == rid
            result = push["response"]["result"]
            assert "path" in result
            assert "values" in result
        finally:
            try:
                await _ws_cancel(ws, [rid])
            except Exception:
                pass
            await ws.close()

    run_async(_test())


def test_ws_close_cancels_subs(ws_url):
    """Closing a WS connection cancels its subscriptions server-side."""

    async def _test():
        # 1. Create sub on ws1 and confirm it delivers
        ws1, rid1 = await _ws_subscribe(ws_url)
        try:
            push = json.loads(
                await asyncio.wait_for(ws1.recv(), timeout=SUB_PUSH_TIMEOUT)
            )
            assert push["response"]["rid"] == rid1, "first push has wrong rid"
        finally:
            await ws1.close()

        # 2. Wait for at least 2 tick cycles so any lingering sub would fire
        await asyncio.sleep(12)

        # 3. Open ws2 and do a one-time read
        ws2 = await websockets.connect(ws_url, open_timeout=WS_TIMEOUT)
        try:
            read_msg = json.dumps({
                "omi": "1.0", "ttl": 0, "read": {"path": SUB_PATH},
            })
            await ws2.send(read_msg)
            resp = json.loads(
                await asyncio.wait_for(ws2.recv(), timeout=WS_TIMEOUT)
            )
            assert resp["response"]["status"] == 200

            # Verify no stale sub deliveries arrive within another tick
            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(ws2.recv(), timeout=SUB_INTERVAL * 2 + 2)
        finally:
            await ws2.close()

    run_async(_test())


def test_ws_multiple_concurrent(ws_url):
    """Multiple concurrent WS connections each receive their own sub pushes."""

    async def _test():
        n_conns = 3
        connections = []

        # Open N connections and subscribe each
        for _ in range(n_conns):
            ws, rid = await _ws_subscribe(ws_url)
            connections.append((ws, rid))

        try:
            # Collect one push from each connection concurrently
            async def _wait_push(ws, rid):
                msg = json.loads(
                    await asyncio.wait_for(ws.recv(), timeout=SUB_PUSH_TIMEOUT)
                )
                assert msg["response"]["rid"] == rid
                assert msg["response"]["status"] == 200
                assert "values" in msg["response"]["result"]
                return msg

            results = await asyncio.gather(
                *[_wait_push(ws, rid) for ws, rid in connections]
            )
            assert len(results) == n_conns
        finally:
            for ws, rid in connections:
                try:
                    await _ws_cancel(ws, [rid])
                except Exception:
                    pass
                try:
                    await ws.close()
                except Exception:
                    pass

    run_async(_test())
