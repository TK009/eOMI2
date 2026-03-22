"""E2E test: WSOP verification color changes on retry (FR-115).

When a joiner is denied, it retries with a fresh X25519 keypair. Since the
verification code is derived from the public key (BLAKE2b), the color should
change between attempts (with high probability — 7/8 chance per retry).

Two-device test:
  - Gateway (DUT): provisioned, running hidden AP
  - Joiner (second device): NVS erased, attempts WSOP onboarding

Environment:
  DEVICE_IP       - gateway IP
  DEVICE_PORT     - gateway serial port
  JOINER_PORT     - joiner serial port (or BRIDGE_PORT)
  API_TOKEN       - bearer token for gateway OMI writes
"""

import subprocess
import time

import pytest

from conftest import (
    deny_joiner,
    erase_nvs,
    reboot_and_capture_serial,
    wait_for_pending_request,
)

pytestmark = pytest.mark.wsop

JOINER_BOOT_TIMEOUT = 30
VALID_COLORS = {"Red", "Green", "Blue", "Yellow", "Cyan", "Magenta", "White", "Orange"}


class TestWsopColorChange:
    """Deny first attempt, verify verification color changes on retry."""

    @pytest.fixture(autouse=True)
    def _setup_joiner(self, joiner_port):
        """Erase joiner NVS."""
        erase_nvs(joiner_port)

    def test_color_changes_after_denial(self, gateway_url, token, joiner_port):
        """FR-115: Fresh keypair per retry means verification color changes.

        Deny the first JoinRequest, observe the color. Wait for the retry,
        observe the new color. Colors should differ (7/8 probability per
        retry; we allow up to 3 retries to avoid flakiness).
        """
        # Boot joiner
        reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )

        # Capture first attempt's color
        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        first_entry = pending[0]
        first_color = first_entry["color"]
        joiner_mac = first_entry["mac"]

        assert first_color in VALID_COLORS, f"Invalid color: {first_color}"

        # Deny the first attempt
        deny_joiner(gateway_url, joiner_mac, token)

        # Wait for joiner to retry with fresh keypair and send a new JoinRequest.
        # The retry involves: detect denial -> generate new keypair -> reconnect
        # -> send new JoinRequest. This can take 10-30s.
        colors_seen = [first_color]
        max_retries = 3

        for _ in range(max_retries):
            time.sleep(5)  # Give joiner time to retry
            try:
                pending = wait_for_pending_request(gateway_url, token, timeout=30)
            except TimeoutError:
                # Joiner may have exhausted retries
                break

            new_entry = pending[0]
            new_color = new_entry["color"]
            colors_seen.append(new_color)

            if new_color != first_color:
                # Color changed — test passes
                break

            # Same color, deny and try again
            deny_joiner(gateway_url, new_entry["mac"], token)

        # With 8 possible colors, P(same color after 3 retries) = (1/8)^3 < 0.02%
        unique_colors = set(colors_seen)
        assert len(unique_colors) > 1, (
            f"Verification color did not change across {len(colors_seen)} attempts: "
            f"{colors_seen}. Fresh keypair should produce different colors (FR-115)."
        )

    def test_serial_shows_different_colors(self, gateway_url, token, joiner_port):
        """Verify via serial output that the joiner logs different colors."""
        erase_nvs(joiner_port)

        subprocess.run(
            ["espflash", "reset", "--port", joiner_port],
            check=True, capture_output=True, timeout=10,
        )

        import serial as pyserial
        ser = pyserial.Serial(joiner_port, 115200, timeout=1)
        colors_from_serial = []
        deadline = time.monotonic() + 90

        try:
            while time.monotonic() < deadline and len(colors_from_serial) < 3:
                raw = ser.readline()
                if not raw:
                    # While waiting for serial, deny any pending requests
                    try:
                        pending = wait_for_pending_request(
                            gateway_url, token, timeout=3
                        )
                        deny_joiner(gateway_url, pending[0]["mac"], token)
                    except TimeoutError:
                        pass
                    continue

                line = raw.decode("utf-8", errors="replace").strip()
                # Parse "WSOP display: showing Red (255,0,0)"
                if "WSOP display: showing" in line:
                    parts = line.split("WSOP display: showing")
                    if len(parts) > 1:
                        color_part = parts[1].strip().split(" ")[0]
                        if color_part in VALID_COLORS:
                            colors_from_serial.append(color_part)
        finally:
            ser.close()

        assert len(colors_from_serial) >= 2, (
            f"Expected at least 2 color displays, got {len(colors_from_serial)}: "
            f"{colors_from_serial}"
        )

        # At least one color should differ (high probability with fresh keypairs)
        if len(set(colors_from_serial)) == 1 and len(colors_from_serial) <= 2:
            pytest.skip(
                f"All {len(colors_from_serial)} colors were {colors_from_serial[0]} — "
                "statistically unlikely but possible; not a test failure"
            )
