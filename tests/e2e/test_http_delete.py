"""Section 4 — HTTP Delete Operations.

Tests for OMI delete: removing user items, forbidden root deletion,
and deleting nonexistent paths.
"""

import pytest
import requests

from helpers import omi_delete, omi_read, omi_write


def test_delete_user_item(base_url, token):
    """Write a user item, delete it, and verify it is gone."""
    # Write /Test/X = 42
    write_data = omi_write(base_url, path="/Test/X", value="42", token=token)
    assert write_data["response"]["status"] in (200, 201)

    try:
        # Verify it was written
        read_data = omi_read(base_url, path="/Test/X", token=token)
        assert read_data["response"]["status"] == 200

        # Delete /Test
        delete_data = omi_delete(base_url, path="/Test", token=token)
        assert delete_data["response"]["status"] == 200

        # Verify /Test is gone
        gone_data = omi_read(base_url, path="/Test", token=token)
        assert gone_data["response"]["status"] == 404
    finally:
        # Best-effort cleanup in case assertions fail before delete succeeds
        try:
            omi_delete(base_url, path="/Test", token=token)
        except Exception:
            pass


def test_delete_root_forbidden(base_url, token):
    """Deleting the root path must be rejected at parse time (HTTP 400)."""
    with pytest.raises(requests.exceptions.HTTPError) as exc_info:
        omi_delete(base_url, path="/", token=token)
    assert exc_info.value.response.status_code == 400


def test_delete_nonexistent(base_url, token):
    """Deleting a path that does not exist must return 404."""
    data = omi_delete(base_url, path="/Ghost", token=token)
    assert data["response"]["status"] == 404
