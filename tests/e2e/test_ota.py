"""OTA firmware update e2e tests.

FR-025, FR-026, SC-007 — build, upload, reboot, verify version cycle.

Tests are ordered and must run sequentially: they flash firmware B, verify
the version change, then restore firmware A to leave the device in its
original state for subsequent test suites.
"""

import pytest
import requests

from helpers import omi_read, omi_status, omi_result, ota_upload, wait_for_device

pytestmark = pytest.mark.ota

# The version string baked into firmware A is CARGO_PKG_VERSION.
# Read it from Cargo.toml so the test stays in sync across version bumps.
def _cargo_pkg_version():
    import tomllib
    from pathlib import Path
    cargo_toml = Path(__file__).resolve().parents[2] / "Cargo.toml"
    with open(cargo_toml, "rb") as f:
        return tomllib.load(f)["package"]["version"]

VERSION_A = _cargo_pkg_version()
# Firmware B is built with FIRMWARE_VERSION=e2e-ota-test.
VERSION_B = "e2e-ota-test"


def _read_firmware_version(base_url, token):
    """Read /System/FirmwareVersion and return the version string."""
    data = omi_read(base_url, path="/System/FirmwareVersion", token=token)
    assert omi_status(data) == 200
    values = omi_result(data)["values"]
    assert len(values) >= 1
    return values[0]["v"]


# -- 1. Read current version (should be A) --------------------------------

def test_read_version_a(base_url, token):
    """Device reports firmware version A before any OTA."""
    version = _read_firmware_version(base_url, token)
    assert version == VERSION_A, f"expected {VERSION_A!r}, got {version!r}"


# -- 2. Reject unauthenticated OTA ----------------------------------------

def test_ota_reject_no_auth(base_url):
    """POST /ota without a token returns 401."""
    resp = requests.post(f"{base_url}/ota", data=b"anything", timeout=10)
    assert resp.status_code == 401


# -- 3. Reject invalid token OTA ------------------------------------------

def test_ota_reject_bad_auth(base_url):
    """POST /ota with an invalid token returns 401."""
    headers = {"Authorization": "Bearer wrong-token"}
    resp = requests.post(
        f"{base_url}/ota", data=b"anything", headers=headers, timeout=10,
    )
    assert resp.status_code == 401


# -- 4. Reject non-gzip payload -------------------------------------------

def test_ota_reject_invalid_payload(base_url, token):
    """POST /ota with neither gzip nor valid ESP firmware returns 400."""
    headers = {
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/octet-stream",
    }
    resp = requests.post(
        f"{base_url}/ota",
        data=b"not gzip firmware",
        headers=headers,
        timeout=10,
    )
    assert resp.status_code == 400
    body = resp.json()
    assert "firmware" in body.get("message", "").lower()


# -- 5-7. Upload firmware B, wait reboot, verify version B ----------------

def test_ota_upload_b_and_verify(base_url, token, ota_firmware_b):
    """Upload firmware B via OTA (raw), wait for reboot, verify new version."""
    # (5) Upload firmware B (raw — avoids ~40 KB gzip decompressor allocation)
    # The device reboots after a successful OTA.  If the reboot occurs before
    # the HTTP response is fully delivered (chunked encoding + TCP flush race),
    # the client sees a ConnectionError.  That is acceptable — the real
    # assertion is the firmware version after reboot.
    try:
        resp = ota_upload(base_url, ota_firmware_b, token)
    except requests.ConnectionError:
        resp = None

    if resp is not None:
        if resp.status_code == 500:
            body = resp.json()
            if "No OTA partition" in body.get("message", ""):
                pytest.skip(
                    "OTA unavailable — DUT firmware exceeds OTA partition size "
                    "(debug build too large for ota_0, corrupts ota_1)"
                )
        assert resp.status_code == 200, f"OTA upload failed: {resp.text}"

    # (6) Wait for reboot
    wait_for_device(base_url, timeout=60)

    # (7) Verify version B
    version = _read_firmware_version(base_url, token)
    assert version == VERSION_B, f"expected {VERSION_B!r}, got {version!r}"


# -- 8. Verify data preservation ------------------------------------------

def test_data_preserved_after_ota(base_url, token):
    """After OTA, device is online and authenticated OMI reads still work."""
    data = omi_read(base_url, path="/System", token=token)
    assert omi_status(data) == 200


# -- 9-10. Restore firmware A and verify ----------------------------------

def test_ota_restore_a_and_verify(base_url, token, ota_firmware_a):
    """Restore firmware A via OTA (raw), wait for reboot, verify original version."""
    # (9) Upload firmware A (raw) — same ConnectionError handling as upload B.
    try:
        resp = ota_upload(base_url, ota_firmware_a, token)
    except requests.ConnectionError:
        resp = None

    if resp is not None:
        if resp.status_code == 500:
            body = resp.json()
            if "No OTA partition" in body.get("message", ""):
                pytest.skip("OTA unavailable — debug firmware exceeds partition size")
        assert resp.status_code == 200, f"OTA upload failed: {resp.text}"

    # Wait for reboot
    wait_for_device(base_url, timeout=60)

    # (10) Verify version A restored
    version = _read_firmware_version(base_url, token)
    assert version == VERSION_A, f"expected {VERSION_A!r}, got {version!r}"
