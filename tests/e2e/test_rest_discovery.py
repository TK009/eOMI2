"""Section 5 — REST Discovery.

GET endpoints under /omi/* that return JSON describing the O-DF tree.

Trailing-slash convention: object paths use a trailing slash (e.g. /omi/Obj/)
while info-item paths do not (e.g. /omi/Obj/Item).
"""

import pytest
import requests

from helpers import REQUEST_TIMEOUT, wait_for_values


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _get_json(url):
    """GET a URL, assert 200, and return parsed JSON."""
    resp = requests.get(url, timeout=REQUEST_TIMEOUT)
    resp.raise_for_status()
    data = resp.json()
    assert data["response"]["status"] == 200
    return data


def _first_object_and_item(base_url):
    """Discover the first object and its first info-item from the tree root."""
    data = _get_json(f"{base_url}/omi/")
    result = data["response"]["result"]
    assert isinstance(result, dict) and len(result) > 0, "OMI tree is empty"
    obj_id = next(iter(result))

    obj_data = _get_json(f"{base_url}/omi/{obj_id}/")
    obj_result = obj_data["response"]["result"]
    items = obj_result["items"]
    assert isinstance(items, dict) and len(items) > 0, f"Object {obj_id} has no items"
    item_id = next(iter(items))

    return obj_id, item_id


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def discovered(base_url):
    """Return (object_id, item_id) discovered dynamically from the device."""
    return _first_object_and_item(base_url)


# ---------------------------------------------------------------------------
# Positive tests
# ---------------------------------------------------------------------------

def test_get_omi_root(base_url):
    """GET /omi/ returns the full object tree with at least one object."""
    data = _get_json(f"{base_url}/omi/")
    assert data["omi"] == "1.0"
    result = data["response"]["result"]
    assert isinstance(result, dict) and len(result) > 0


def test_get_omi_object(base_url, discovered):
    """GET /omi/<object>/ returns the object with its info-items."""
    obj_id, _ = discovered
    data = _get_json(f"{base_url}/omi/{obj_id}/")
    result = data["response"]["result"]
    assert result["id"] == obj_id
    assert isinstance(result["items"], dict) and len(result["items"]) > 0


def test_get_omi_item(base_url, discovered):
    """GET /omi/<object>/<item> returns an info-item with a values array."""
    obj_id, item_id = discovered
    data = _get_json(f"{base_url}/omi/{obj_id}/{item_id}")
    result = data["response"]["result"]
    values = result["values"]
    assert isinstance(values, list)
    if len(values) > 0:
        entry = values[0]
        assert "v" in entry, f"value entry missing 'v' key: {entry}"


def test_get_omi_query_newest(base_url, discovered):
    """GET /omi/<object>/<item>?newest=2 returns 1-2 values."""
    obj_id, item_id = discovered
    # Ensure at least one sensor reading exists (may not after a reboot)
    wait_for_values(base_url, path=f"/{obj_id}/{item_id}")
    data = _get_json(f"{base_url}/omi/{obj_id}/{item_id}?newest=2")
    values = data["response"]["result"]["values"]
    assert isinstance(values, list)
    assert 1 <= len(values) <= 2


# ---------------------------------------------------------------------------
# Negative / edge-case tests
# ---------------------------------------------------------------------------

def test_get_nonexistent_object(base_url):
    """GET /omi/NonExistent/ returns an error status."""
    resp = requests.get(f"{base_url}/omi/NonExistent/", timeout=REQUEST_TIMEOUT)
    # Accept either HTTP-level or OMI-level error
    if resp.status_code == 200:
        data = resp.json()
        assert data["response"]["status"] != 200, (
            "Expected an error for a nonexistent object"
        )
    else:
        assert resp.status_code == 404


def test_get_nonexistent_item(base_url, discovered):
    """GET /omi/<object>/NoSuchItem returns an error status."""
    obj_id, _ = discovered
    resp = requests.get(
        f"{base_url}/omi/{obj_id}/NoSuchItem", timeout=REQUEST_TIMEOUT
    )
    if resp.status_code == 200:
        data = resp.json()
        assert data["response"]["status"] != 200, (
            "Expected an error for a nonexistent item"
        )
    else:
        assert resp.status_code == 404


def test_get_newest_zero(base_url, discovered):
    """GET ?newest=0 returns an empty values list."""
    obj_id, item_id = discovered
    resp = requests.get(
        f"{base_url}/omi/{obj_id}/{item_id}?newest=0", timeout=REQUEST_TIMEOUT
    )
    resp.raise_for_status()
    data = resp.json()
    values = data["response"]["result"]["values"]
    assert isinstance(values, list)
    assert len(values) == 0


def test_get_newest_negative(base_url, discovered):
    """GET ?newest=-1 returns an error or a valid result (firmware-defined)."""
    obj_id, item_id = discovered
    resp = requests.get(
        f"{base_url}/omi/{obj_id}/{item_id}?newest=-1", timeout=REQUEST_TIMEOUT
    )
    if resp.status_code == 200:
        data = resp.json()
        # Accept either an OMI error or a valid result — firmware may treat
        # negative values as unsigned or clamp them.
        if data["response"]["status"] == 200:
            values = data["response"]["result"]["values"]
            assert isinstance(values, list)
    else:
        assert resp.status_code in (400, 422)
