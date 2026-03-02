"""Shared fixtures for e2e tests."""

import os

import pytest


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
