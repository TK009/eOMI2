"""Section 5 — REST Discovery.

GET endpoints under /omi/* that return JSON describing the O-DF tree.
"""

import requests

from helpers import REQUEST_TIMEOUT


def test_get_omi_root(base_url):
    """GET /omi/ returns the full object tree with Dht11."""
    resp = requests.get(f"{base_url}/omi/", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    data = resp.json()
    assert data["omi"] == "1.0"
    assert data["response"]["status"] == 200
    result = data["response"]["result"]
    assert "Dht11" in result


def test_get_omi_object(base_url):
    """GET /omi/Dht11/ returns the object with its info-items."""
    resp = requests.get(f"{base_url}/omi/Dht11/", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    data = resp.json()
    assert data["response"]["status"] == 200
    result = data["response"]["result"]
    assert result["id"] == "Dht11"
    assert "Temperature" in result["items"]
    assert "RelativeHumidity" in result["items"]


def test_get_omi_item(base_url):
    """GET /omi/Dht11/Temperature returns an info-item with a values array."""
    resp = requests.get(
        f"{base_url}/omi/Dht11/Temperature", timeout=REQUEST_TIMEOUT
    )
    assert resp.status_code == 200
    data = resp.json()
    assert data["response"]["status"] == 200
    result = data["response"]["result"]
    assert isinstance(result["values"], list)


def test_get_omi_query_newest(base_url):
    """GET /omi/Dht11/Temperature?newest=2 returns at most 2 values."""
    resp = requests.get(
        f"{base_url}/omi/Dht11/Temperature?newest=2", timeout=REQUEST_TIMEOUT
    )
    assert resp.status_code == 200
    data = resp.json()
    assert data["response"]["status"] == 200
    values = data["response"]["result"]["values"]
    assert isinstance(values, list)
    assert len(values) <= 2
