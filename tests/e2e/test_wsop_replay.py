"""E2E test: WSOP replay attack rejection (FR-122).

Verifies that the gateway rejects a JOIN_REQUEST with a stale timestamp
(drift > 300s). This is tested by writing a crafted base64-encoded
JoinRequest directly to the gateway's JoinRequest InfoItem from the test
host, using a timestamp far in the past.

Single-device test: only the gateway (DUT) is needed.

Environment:
  DEVICE_IP   - gateway IP
  API_TOKEN   - bearer token for gateway OMI writes
"""

import json
import os
import time

import pytest

from helpers import omi_read, omi_write, REQUEST_TIMEOUT

pytestmark = pytest.mark.wsop

JOIN_REQUEST_PATH = "/Objects/OnboardingGateway/JoinRequest"
PENDING_REQUESTS_PATH = "/Objects/OnboardingGateway/PendingRequests"


def build_stale_join_request_b64(timestamp_offset_secs=-600):
    """Build a base64-encoded JoinRequest with a stale timestamp.

    Uses the WSOP wire format:
      version(1) + name_len(1) + name + mac(6) + pubkey(32) + nonce(8) + timestamp(4)

    The timestamp is set to (current_time + offset), so a negative offset
    makes it stale.
    """
    import base64
    import struct

    version = 0x01
    name = b"replay-test"
    mac = bytes([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01])
    pubkey = bytes(range(32))  # deterministic fake pubkey
    nonce = bytes([0xAA] * 8)
    # Use a timestamp far in the past (stale)
    stale_time = max(0, int(time.time()) + timestamp_offset_secs)

    buf = bytearray()
    buf.append(version)
    buf.append(len(name))
    buf.extend(name)
    buf.extend(mac)
    buf.extend(pubkey)
    buf.extend(nonce)
    buf.extend(struct.pack(">I", stale_time))

    return base64.b64encode(bytes(buf)).decode("ascii")


def get_pending_count(gateway_url, token):
    """Read PendingRequests and return the number of pending entries."""
    try:
        data = omi_read(gateway_url, PENDING_REQUESTS_PATH, token=token)
        if data.get("response", {}).get("status") == 200:
            result = data["response"]["result"]
            values = result.get("values", [])
            if values:
                raw = values[0].get("v", "[]")
                pending = json.loads(raw) if isinstance(raw, str) else raw
                return len(pending)
    except (KeyError, json.JSONDecodeError, TypeError):
        pass
    return 0


class TestWsopReplay:
    """Replay old JOIN_REQUEST with stale timestamp; verify gateway rejects."""

    def test_stale_timestamp_rejected(self, gateway_url, token):
        """FR-122: Gateway rejects JoinRequest with timestamp drift > 300s.

        Write a crafted JoinRequest with a timestamp 600s in the past directly
        to the gateway's JoinRequest InfoItem. The gateway's onwrite handler
        should reject it (PendingRequests should not grow).
        """
        # Record current pending count
        initial_count = get_pending_count(gateway_url, token)

        # Write a stale JoinRequest (600s in the past)
        stale_b64 = build_stale_join_request_b64(timestamp_offset_secs=-600)
        resp = omi_write(
            gateway_url, JOIN_REQUEST_PATH, stale_b64, token=token
        )
        # The write itself may succeed (InfoItem accepts the value), but the
        # gateway's processing logic should reject it and NOT add to pending.
        # Give the gateway a moment to process.
        time.sleep(2)

        # Verify pending count did not increase
        new_count = get_pending_count(gateway_url, token)
        assert new_count == initial_count, (
            f"Gateway accepted stale JoinRequest: pending went from "
            f"{initial_count} to {new_count}"
        )

    def test_future_timestamp_rejected(self, gateway_url, token):
        """FR-122: Gateway also rejects JoinRequest with timestamp too far in
        the future (drift > 300s)."""
        initial_count = get_pending_count(gateway_url, token)

        # Write a JoinRequest with timestamp 600s in the future
        future_b64 = build_stale_join_request_b64(timestamp_offset_secs=600)
        omi_write(gateway_url, JOIN_REQUEST_PATH, future_b64, token=token)
        time.sleep(2)

        new_count = get_pending_count(gateway_url, token)
        assert new_count == initial_count, (
            f"Gateway accepted future-dated JoinRequest: pending went from "
            f"{initial_count} to {new_count}"
        )

    def test_valid_timestamp_accepted(self, gateway_url, token):
        """Sanity check: a JoinRequest with a current timestamp IS accepted."""
        initial_count = get_pending_count(gateway_url, token)

        # Timestamp within +-300s window (offset=0 means "now")
        valid_b64 = build_stale_join_request_b64(timestamp_offset_secs=0)
        omi_write(gateway_url, JOIN_REQUEST_PATH, valid_b64, token=token)
        time.sleep(2)

        new_count = get_pending_count(gateway_url, token)
        assert new_count == initial_count + 1, (
            f"Gateway rejected valid JoinRequest: pending stayed at {new_count} "
            f"(expected {initial_count + 1})"
        )
