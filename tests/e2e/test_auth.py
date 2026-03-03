"""Section 9 — Authentication E2E Tests.

Tests that mutating operations (write, delete) require a valid Bearer
token while reads and REST GETs remain public.
"""

import pytest
import requests

from helpers import REQUEST_TIMEOUT, omi_delete, omi_read, omi_write


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove /Test after all auth tests."""
    yield
    try:
        omi_delete(base_url, "/Test", token=token)
    except Exception:
        pass


def test_read_no_auth(base_url):
    """Read requests succeed without any auth token."""
    data = omi_read(base_url, "/", token=None)
    assert data["response"]["status"] == 200


def test_write_no_auth(base_url):
    """Write without a token returns OMI 401."""
    data = omi_write(base_url, "/Test/AuthNoTok", 1, token=None)
    assert data["response"]["status"] == 401


def test_write_wrong_token(base_url):
    """Write with an invalid token returns OMI 401."""
    data = omi_write(base_url, "/Test/AuthBadTok", 1, token="wrong-token-xxx")
    assert data["response"]["status"] == 401


def test_write_correct_token(base_url, token):
    """Write with the correct token succeeds; value is readable."""
    data = omi_write(base_url, "/Test/AuthOk", 1, token=token)
    assert data["response"]["status"] in (200, 201)

    read = omi_read(base_url, "/Test/AuthOk", token=token, newest=1)
    assert read["response"]["status"] == 200
    values = read["response"]["result"]["values"]
    assert values[0]["v"] == 1


def test_delete_no_auth(base_url):
    """Delete without a token returns OMI 401."""
    data = omi_delete(base_url, "/Test", token=None)
    assert data["response"]["status"] == 401


def test_rest_get_no_auth(base_url):
    """REST GET on /omi/ is public — no auth header needed."""
    resp = requests.get(f"{base_url}/omi/", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    data = resp.json()
    assert data["response"]["status"] == 200
