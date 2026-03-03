"""WebSocket endpoint reachability and basic OMI read over WS.

Verifies that /omi/ws is reachable even though the GET /omi/* wildcard is
also registered.  This catches handler-registration-order regressions
(ESP_ERR_HTTPD_HANDLER_EXISTS).
"""

import asyncio
import json

import pytest
import websockets


WS_TIMEOUT = 10  # seconds


@pytest.fixture(scope="session")
def ws_url(device_ip):
    return f"ws://{device_ip}/omi/ws"


def _run(coro):
    """Run an async coroutine synchronously."""
    return asyncio.get_event_loop().run_until_complete(coro)


async def _ws_read(ws_url, path="/"):
    """Open a WS connection, send a one-time OMI read, return parsed response."""
    async with websockets.connect(ws_url, open_timeout=WS_TIMEOUT) as ws:
        msg = json.dumps({"omi": "1.0", "ttl": 0, "read": {"path": path}})
        await ws.send(msg)
        resp = await asyncio.wait_for(ws.recv(), timeout=WS_TIMEOUT)
        return json.loads(resp)


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
