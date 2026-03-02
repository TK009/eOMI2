"""Section 1 — Boot and Connectivity.

These tests gate every other e2e section: if the device has not booted,
connected to Wi-Fi, and started its HTTP server, nothing else can run.
"""

import requests

from helpers import omi_read, REQUEST_TIMEOUT


def test_device_boots(base_url):
    """Device is reachable over HTTP (booted + Wi-Fi + HTTP server)."""
    resp = requests.get(base_url, timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200


def test_landing_page(base_url):
    """Landing page renders the expected HTML structure."""
    resp = requests.get(base_url, timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    assert "text/html" in resp.headers.get("Content-Type", "")
    body = resp.text
    assert "<h1>Reconfigurable Device</h1>" in body
    assert "Status: running" in body


def test_omi_endpoint_reachable(base_url):
    """OMI endpoint accepts a read request and returns a valid envelope."""
    data = omi_read(base_url, path="/")
    assert data["omi"] == "1.0"
    assert data["response"]["status"] == 200
    result = data["response"]["result"]
    # Verify the tree has at least one child node (structure check,
    # not tied to a specific sensor model).
    assert isinstance(result, (list, dict)), f"unexpected result type: {type(result)}"
    assert len(result) > 0, "OMI root tree is empty"
