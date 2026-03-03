"""Section 8 — Stored Pages.

Verify PATCH/GET/DELETE on arbitrary paths for user-uploaded HTML pages.
"""

import pytest
import requests

REQUEST_TIMEOUT = 10

TEST_PATH = "/testpage"
TEST_HTML = "<h1>Test Page</h1>"


@pytest.fixture(scope="module", autouse=True)
def cleanup_test_page(base_url, auth_headers):
    """Best-effort cleanup: delete the test page after all tests in this module."""
    yield
    try:
        requests.delete(
            f"{base_url}{TEST_PATH}",
            headers=auth_headers,
            timeout=REQUEST_TIMEOUT,
        )
    except Exception:
        pass


def test_store_page(base_url, auth_headers):
    """PATCH /testpage with HTML body stores the page (200)."""
    resp = requests.patch(
        f"{base_url}{TEST_PATH}",
        data=TEST_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200


def test_retrieve_page(base_url, auth_headers):
    """Store a page then GET it back — body matches and Content-Type is text/html."""
    # Store
    requests.patch(
        f"{base_url}{TEST_PATH}",
        data=TEST_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Retrieve
    resp = requests.get(f"{base_url}{TEST_PATH}", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    assert resp.text == TEST_HTML
    assert "text/html" in resp.headers.get("Content-Type", "")


def test_landing_lists_page(base_url, auth_headers):
    """After storing a page, GET / includes a link to it."""
    # Store
    requests.patch(
        f"{base_url}{TEST_PATH}",
        data=TEST_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Check landing page
    resp = requests.get(f"{base_url}/", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 200
    assert TEST_PATH in resp.text


def test_delete_page(base_url, auth_headers):
    """Store a page, DELETE it, then GET returns 404."""
    # Store
    requests.patch(
        f"{base_url}{TEST_PATH}",
        data=TEST_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Delete
    resp = requests.delete(
        f"{base_url}{TEST_PATH}",
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200

    # Verify gone
    resp = requests.get(f"{base_url}{TEST_PATH}", timeout=REQUEST_TIMEOUT)
    assert resp.status_code == 404


def test_store_requires_auth(base_url):
    """PATCH without auth header is rejected with 401."""
    resp = requests.patch(
        f"{base_url}{TEST_PATH}",
        data=TEST_HTML,
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 401
