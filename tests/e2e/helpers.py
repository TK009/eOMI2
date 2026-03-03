"""OMI helper functions for e2e tests."""

import asyncio
import time

import pytest
import requests

REQUEST_TIMEOUT = 10  # seconds – avoid hanging on unresponsive devices
WS_TIMEOUT = 10  # seconds – WebSocket operation timeout


def run_async(coro):
    """Run an async coroutine synchronously."""
    return asyncio.run(coro)


def _omi_post(base_url, payload, token=None, timeout=None, check=True):
    """POST *payload* to the OMI endpoint and return parsed JSON.

    When *check* is True (default) ``raise_for_status()`` is called so
    that unexpected HTTP-level errors surface immediately.  Pass
    ``check=False`` when the test expects a non-200 HTTP status.
    """
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers=headers,
        timeout=REQUEST_TIMEOUT if timeout is None else timeout,
    )
    if check:
        resp.raise_for_status()
    return resp.json()


def omi_read(base_url, path="/", token=None, **read_params):
    """Send an OMI read request and return the parsed JSON response."""
    read_body = {"path": path}
    read_body.update(read_params)
    payload = {"omi": "1.0", "ttl": 0, "read": read_body}
    return _omi_post(base_url, payload, token=token)


def omi_write(base_url, path, value, token=None):
    """Send an OMI write request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "write": {"path": path, "v": value}}
    return _omi_post(base_url, payload, token=token)


def omi_write_batch(base_url, items, token=None):
    """Send an OMI batch write request and return the parsed JSON response.

    *items* is a list of dicts, each with ``path`` and ``v`` keys.
    """
    payload = {"omi": "1.0", "ttl": 0, "write": {"items": items}}
    return _omi_post(base_url, payload, token=token)


def omi_write_tree(base_url, path, objects, token=None, timeout=None):
    """Send an OMI tree write request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "write": {"path": path, "objects": objects}}
    return _omi_post(base_url, payload, token=token, timeout=timeout)


def omi_delete(base_url, path, token=None):
    """Send an OMI delete request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "delete": {"path": path}}
    return _omi_post(base_url, payload, token=token)


def omi_subscribe(base_url, path, interval=-1, ttl=60, token=None):
    """Create a poll subscription (no callback → poll target). Returns parsed JSON."""
    payload = {"omi": "1.0", "ttl": ttl, "read": {"path": path, "interval": interval}}
    return _omi_post(base_url, payload, token=token)


def omi_poll(base_url, rid, token=None, check=True):
    """Poll a subscription by rid. Returns parsed JSON."""
    payload = {"omi": "1.0", "ttl": 10, "read": {"rid": rid}}
    return _omi_post(base_url, payload, token=token, check=check)


def omi_cancel(base_url, rids, token=None):
    """Cancel subscriptions by rid list. Returns parsed JSON."""
    payload = {"omi": "1.0", "ttl": 10, "cancel": {"rid": rids}}
    return _omi_post(base_url, payload, token=token)


# -- Response helpers --------------------------------------------------------

def omi_status(data):
    """Extract the OMI-level status code from a response envelope."""
    return data["response"]["status"]


def omi_result(data):
    """Extract the result payload from a response envelope."""
    return data["response"]["result"]


# -- Polling helpers ---------------------------------------------------------

POLL_BACKOFF = [1, 2, 3, 5, 5, 5]


def wait_for_values(base_url, path="/System/FreeHeap", min_count=1,
                    delays=POLL_BACKOFF):
    """Poll *path* with increasing back-off until at least *min_count* values exist."""
    for delay in delays:
        time.sleep(delay)
        try:
            data = omi_read(base_url, path=path)
        except requests.RequestException:
            continue
        if omi_status(data) == 200 and len(omi_result(data)["values"]) >= min_count:
            return omi_result(data)["values"]
    total = sum(delays)
    pytest.fail(f"No values at {path} after {total}s")
