"""Shared fixtures and OMI helpers for e2e tests."""

import os

import pytest
import requests

REQUEST_TIMEOUT = 10  # seconds – avoid hanging on unresponsive devices


# ---------------------------------------------------------------------------
# Fixtures (session-scoped)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def device_ip():
    """Device IP address from the DEVICE_IP env var."""
    ip = os.environ.get("DEVICE_IP")
    if not ip:
        pytest.skip("DEVICE_IP not set")
    return ip


@pytest.fixture(scope="session")
def base_url(device_ip):
    """Root URL for HTTP requests."""
    return f"http://{device_ip}"


@pytest.fixture(scope="session")
def token():
    """API bearer token from the API_TOKEN env var."""
    tok = os.environ.get("API_TOKEN")
    if not tok:
        pytest.skip("API_TOKEN not set")
    return tok


@pytest.fixture(scope="session")
def auth_headers(token):
    """Authorization header dict."""
    return {"Authorization": f"Bearer {token}"}


# ---------------------------------------------------------------------------
# OMI helper functions
# ---------------------------------------------------------------------------

def omi_read(base_url, path="/"):
    """Send an OMI read request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "read": {"path": path}}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()


def omi_write(base_url, path, value, token):
    """Send an OMI write request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "write": {"path": path, "value": value}}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers={"Authorization": f"Bearer {token}"},
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()


def omi_delete(base_url, path, token):
    """Send an OMI delete request and return the parsed JSON response."""
    payload = {"omi": "1.0", "ttl": 0, "delete": {"path": path}}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers={"Authorization": f"Bearer {token}"},
        timeout=REQUEST_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()
