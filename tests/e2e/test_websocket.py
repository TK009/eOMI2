"""WebSocket e2e tests — reachability, reads, subscriptions, and concurrency.

Verifies that /omi/ws is reachable even though the GET /omi/* wildcard is
also registered.  This catches handler-registration-order regressions
(ESP_ERR_HTTPD_HANDLER_EXISTS).

Subscription tests use interval-based subscriptions (interval>0) rather than
event-based (interval=-1) because the firmware does not call notify_event()
after HTTP writes.  Interval subs fire via tick() in the main loop which
sleeps ~5s between cycles.
"""

import asyncio
import json

import pytest
import websockets


WS_TIMEOUT = 10  # seconds
SUB_PUSH_TIMEOUT = 20  # seconds — wait for interval sub delivery (~2-4 ticks)
SUB_PATH = "/Dht11/Temperature"
SUB_INTERVAL = 5  # seconds — matches main loop tick period
SUB_TTL = 60


@pytest.fixture(scope="session")
def ws_url(device_ip):
    return f"ws://{device_ip}/omi/ws"


def _run(coro):
    """Run an async coroutine synchronously."""
    return asyncio.get_event_loop().run_until_complete(coro)


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
    data = _run(_ws_read(ws_url))
    assert data["omi"] == "1.0"
    assert data["response"]["status"] == 200


def test_ws_read_root(ws_url):
    """OMI read over WS returns the object tree root."""
    data = _run(_ws_read(ws_url, path="/"))
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

    _run(_test())


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
                await asyncio.wait_for(ws2.recv(), timeout=8)
        finally:
            await ws2.close()

    _run(_test())


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

    _run(_test())
