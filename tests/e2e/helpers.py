"""OMI helper functions for e2e tests."""

import requests

REQUEST_TIMEOUT = 10  # seconds – avoid hanging on unresponsive devices


def omi_read(base_url, path="/", token=None, **read_params):
    """Send an OMI read request and return the parsed JSON response."""
    read_body = {"path": path}
    read_body.update(read_params)
    payload = {"omi": "1.0", "ttl": 0, "read": read_body}
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()


def omi_write(base_url, path, value, token=None):
    """Send an OMI write request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "write": {"path": path, "value": value}}
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()


def omi_delete(base_url, path, token=None):
    """Send an OMI delete request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "delete": {"path": path}}
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()
