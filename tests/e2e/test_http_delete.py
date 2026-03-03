"""Section 4 — HTTP Delete Operations.

Tests for OMI delete: removing user items, forbidden root deletion,
and deleting nonexistent paths.
"""

import requests

from helpers import omi_delete, omi_read, omi_write, REQUEST_TIMEOUT


def test_delete_user_item(base_url, token):
    """Write a user item, delete it, and verify it is gone."""
    # Write /Test/X = 42
    write_data = omi_write(base_url, path="/Test/X", value="42", token=token)
    assert write_data["response"]["status"] in (200, 201)

    # Verify it was written
    read_data = omi_read(base_url, path="/Test/X", token=token)
    assert read_data["response"]["status"] == 200

    # Delete /Test
    delete_data = omi_delete(base_url, path="/Test", token=token)
    assert delete_data["response"]["status"] == 200

    # Verify /Test is gone
    payload = {"omi": "1.0", "ttl": 0, "read": {"path": "/Test"}}
    headers = {"Authorization": f"Bearer {token}"}
    resp = requests.post(
        f"{base_url}/omi", json=payload, headers=headers, timeout=REQUEST_TIMEOUT
    )
    data = resp.json()
    assert data["response"]["status"] == 404


def test_delete_root_forbidden(base_url, token):
    """Deleting the root path must be rejected."""
    payload = {"omi": "1.0", "ttl": 0, "delete": {"path": "/"}}
    headers = {"Authorization": f"Bearer {token}"}
    resp = requests.post(
        f"{base_url}/omi", json=payload, headers=headers, timeout=REQUEST_TIMEOUT
    )
    assert resp.status_code == 400
    assert "cannot delete root" in resp.text.lower()


def test_delete_nonexistent(base_url, token):
    """Deleting a path that does not exist must return 404."""
    payload = {"omi": "1.0", "ttl": 0, "delete": {"path": "/Ghost"}}
    headers = {"Authorization": f"Bearer {token}"}
    resp = requests.post(
        f"{base_url}/omi", json=payload, headers=headers, timeout=REQUEST_TIMEOUT
    )
    data = resp.json()
    assert data["response"]["status"] == 404
