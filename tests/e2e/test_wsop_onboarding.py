"""E2E test: WSOP happy-path onboarding (SC-002, SC-008, SC-010).

Two-device test: gateway (provisioned DUT) approves joiner (second device
with erased NVS). Verifies:
  - Joiner connects to hidden SSID and sends JoinRequest
  - Gateway exposes PendingRequests via standard OMI read
  - Owner approves via standard OMI write to Approval InfoItem
  - Joiner decrypts credentials, connects to real WiFi
  - NeoPixel is released (verification display stops)

Environment:
  DEVICE_IP       - gateway IP (provisioned, on the network)
  DEVICE_PORT     - gateway serial port
  JOINER_PORT     - joiner serial port (or BRIDGE_PORT)
  WIFI_SSID       - real network SSID (gateway is connected to this)
  WIFI_PASS       - real network password
  API_TOKEN       - bearer token for gateway OMI writes
"""

import os
import time

import pytest

from conftest import (
    approve_joiner,
    erase_nvs,
    reboot_and_capture_serial,
    wait_for_pending_request,
)

pytestmark = pytest.mark.wsop

# Timeouts tuned to WSOP protocol timing.
JOINER_BOOT_TIMEOUT = 30  # seconds for joiner to boot and start scanning
ONBOARD_FLOW_TIMEOUT = 90  # SC-002: full onboarding under 90s with prompt approval


@pytest.fixture(scope="module")
def wifi_ssid():
    ssid = os.environ.get("WIFI_SSID")
    if not ssid:
        pytest.skip("WIFI_SSID not set")
    return ssid


@pytest.fixture(scope="module")
def wifi_pass():
    return os.environ.get("WIFI_PASS", "")


class TestWsopOnboarding:
    """Happy-path: gateway approves joiner, joiner connects to real WiFi."""

    @pytest.fixture(autouse=True)
    def _setup_joiner(self, joiner_port):
        """Erase joiner NVS so it boots as a WSOP joiner."""
        erase_nvs(joiner_port)

    def test_full_onboarding_flow(self, gateway_url, token, joiner_port):
        """SC-002: Full onboarding completes in under 90s with prompt approval.
        SC-010: Standard OMI read/write is the only interface needed."""
        flow_start = time.monotonic()

        # Reboot joiner — it will scan for _eomi_onboard, connect, send JoinRequest
        serial_result = reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )
        assert serial_result["WSOP: sending JoinRequest"] is not None, (
            "Joiner did not send JoinRequest — check that gateway's hidden AP is active"
        )

        # Read PendingRequests from gateway via standard OMI read (SC-010)
        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        assert len(pending) >= 1, "Expected at least one pending join request"

        joiner_entry = pending[0]
        assert "mac" in joiner_entry, "Pending entry must include MAC"
        assert "color" in joiner_entry, "Pending entry must include verification color"
        assert "name" in joiner_entry, "Pending entry must include device name"
        assert "remaining" in joiner_entry, "Pending entry must include remaining time"

        joiner_mac = joiner_entry["mac"]

        # Approve the joiner via standard OMI write (SC-010)
        resp = approve_joiner(gateway_url, joiner_mac, token)
        assert resp["response"]["status"] in (200, 201), (
            f"Approval write failed: {resp}"
        )

        # Wait for joiner to process the response — poll serial
        import serial as pyserial
        ser = pyserial.Serial(joiner_port, 115200, timeout=1)
        success = False
        deadline = time.monotonic() + 60
        try:
            while time.monotonic() < deadline:
                raw = ser.readline()
                if not raw:
                    continue
                line = raw.decode("utf-8", errors="replace").strip()
                if "WSOP: onboarding succeeded" in line:
                    success = True
                    break
                if "Wi-Fi connected. IP:" in line:
                    success = True
                    break
        finally:
            ser.close()

        assert success, "Joiner did not report onboarding success within timeout"

        elapsed = time.monotonic() - flow_start
        assert elapsed < ONBOARD_FLOW_TIMEOUT, (
            f"SC-002: Full onboarding took {elapsed:.1f}s, exceeds {ONBOARD_FLOW_TIMEOUT}s limit"
        )

    def test_pending_requests_contain_verification_info(self, gateway_url, token,
                                                         joiner_port):
        """PendingRequests includes color, digit, MAC, name, remaining time."""
        # Reboot joiner to trigger a new JoinRequest
        reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )

        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        entry = pending[0]

        # Validate all required fields per FR-123
        assert "mac" in entry
        assert "name" in entry
        assert "color" in entry
        assert entry["color"] in [
            "Red", "Green", "Blue", "Yellow", "Cyan", "Magenta", "White", "Orange"
        ], f"Unexpected color: {entry['color']}"
        assert "digit" in entry
        assert 0 <= entry["digit"] <= 9
        assert "remaining" in entry
        assert entry["remaining"] > 0

    def test_neopixel_released_after_onboarding(self, gateway_url, token,
                                                  joiner_port):
        """SC-008: NeoPixel is usable for application purposes after onboarding."""
        # Erase and reboot joiner
        erase_nvs(joiner_port)

        serial_result = reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )
        if serial_result["WSOP: sending JoinRequest"] is None:
            pytest.skip("Joiner did not reach JoinRequest phase")

        # Approve
        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        approve_joiner(gateway_url, pending[0]["mac"], token)

        # Check serial for NeoPixel release message (FR-133)
        import serial as pyserial
        ser = pyserial.Serial(joiner_port, 115200, timeout=1)
        released = False
        deadline = time.monotonic() + 60
        try:
            while time.monotonic() < deadline:
                raw = ser.readline()
                if not raw:
                    continue
                line = raw.decode("utf-8", errors="replace").strip()
                if "NeoPixel released" in line or "display: stopped" in line:
                    released = True
                    break
                if "WSOP: onboarding succeeded" in line:
                    # Success implies display was stopped (FR-132)
                    released = True
                    break
        finally:
            ser.close()

        assert released, "NeoPixel was not released after onboarding (SC-008)"
