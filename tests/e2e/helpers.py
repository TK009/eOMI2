"""OMI helper functions for e2e tests."""

import asyncio
import subprocess
import time

import pytest
import requests

REQUEST_TIMEOUT = 15  # seconds – ESP32-S2 full tree reads can take 7+ seconds
OTA_TIMEOUT = 120  # seconds – generous for ~1.2 MB compressed over LAN
TREE_WRITE_TIMEOUT = 45  # seconds – tree writes with metadata need more time on ESP32-S2
WS_TIMEOUT = 20  # seconds – WebSocket operation timeout (generous for device under load)
DEVICE_RETRIES = 3  # retry count for transient connection resets (ESP32 has limited sockets)


def device_get(url, **kwargs):
    """GET with retry — ESP32 HTTP server has limited sockets and may reset connections."""
    kwargs.setdefault("timeout", REQUEST_TIMEOUT)
    last_err = None
    for attempt in range(DEVICE_RETRIES):
        try:
            return requests.get(url, **kwargs)
        except (requests.ConnectionError, requests.exceptions.ReadTimeout) as e:
            last_err = e
            time.sleep(1)
    raise last_err


def run_async(coro):
    """Run an async coroutine synchronously."""
    return asyncio.run(coro)


def ota_upload(base_url, firmware_path, token):
    """Upload firmware via POST /ota (gzip-compressed or raw)."""
    with open(firmware_path, "rb") as f:
        data = f.read()
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/octet-stream",
    }
    return requests.post(
        f"{base_url}/ota", data=data, headers=headers, timeout=OTA_TIMEOUT,
    )


def reboot_device(device_port):
    """Trigger a hardware reset via espflash."""
    subprocess.run(
        ["espflash", "reset", "--port", device_port],
        check=True,
        capture_output=True,
        timeout=10,
    )


def wait_for_device_down(base_url, timeout=10):
    """Poll GET / until the device stops responding (connection refused/timeout)."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            requests.get(base_url, timeout=2)
        except requests.RequestException:
            return  # device is down
        time.sleep(0.5)
    raise TimeoutError(f"Device did not go offline within {timeout}s")


def wait_for_device(base_url, timeout=30, readiness_path="/System"):
    """Poll the device until it is fully ready.

    Checks that the OMI subsystem is up by reading *readiness_path*
    (default ``/System``) and verifying a 200 OMI-level status.  Falls
    back to a simple HTTP 200 check on ``/`` when *readiness_path* is
    ``None``.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if readiness_path is not None:
                data = _omi_post(
                    base_url,
                    {"omi": "1.0", "ttl": 0, "read": {"path": readiness_path}},
                    check=False,
                    timeout=5,
                )
                if data.get("response", {}).get("status") == 200:
                    return
            else:
                resp = requests.get(base_url, timeout=5)
                if resp.status_code == 200:
                    return
        except (requests.RequestException, ValueError):
            pass
        time.sleep(1)
    raise TimeoutError(f"Device did not become reachable within {timeout}s")


def _omi_post(base_url, payload, token=None, timeout=None, check=True):
    """POST *payload* to the OMI endpoint and return parsed JSON.

    When *check* is True (default) ``raise_for_status()`` is called so
    that unexpected HTTP-level errors surface immediately.  Pass
    ``check=False`` when the test expects a non-200 HTTP status.
    """
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    t = REQUEST_TIMEOUT if timeout is None else timeout
    last_err = None
    for _attempt in range(DEVICE_RETRIES):
        try:
            resp = requests.post(
                f"{base_url}/omi",
                json=payload,
                headers=headers,
                timeout=t,
            )
            if check:
                resp.raise_for_status()
            return resp.json()
        except (requests.ConnectionError, requests.exceptions.ReadTimeout) as e:
            last_err = e
            time.sleep(1)
    raise last_err


def omi_read(base_url, path="/", token=None, timeout=None, **read_params):
    """Send an OMI read request and return the parsed JSON response."""
    read_body = {"path": path}
    read_body.update(read_params)
    payload = {"omi": "1.0", "ttl": 0, "read": read_body}
    return _omi_post(base_url, payload, token=token, timeout=timeout)


def omi_write(base_url, path, value, token=None, timeout=None):
    """Send an OMI write request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "write": {"path": path, "v": value}}
    return _omi_post(base_url, payload, token=token, timeout=timeout)


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
