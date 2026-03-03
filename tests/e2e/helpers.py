"""OMI helper functions for e2e tests."""

import asyncio

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


def omi_read(base_url, path="/", token=None, newest=None):
    """Send an OMI read request and return the parsed JSON response."""
    read_obj = {"path": path}
    if newest is not None:
        read_obj["newest"] = newest
    payload = {"omi": "1.0", "ttl": 0, "read": read_obj}
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
