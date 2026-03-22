"""E2E test: WSOP denial and timeout fallback (SC-004).

Verifies that when the gateway denies a joiner (or lets the approval
window expire), the joiner falls back to captive portal provisioning.

Two-device test:
  - Gateway (DUT): provisioned, running hidden AP
  - Joiner (second device): NVS erased, attempts WSOP onboarding

Environment:
  DEVICE_IP       - gateway IP
  DEVICE_PORT     - gateway serial port
  JOINER_PORT     - joiner serial port (or BRIDGE_PORT)
  API_TOKEN       - bearer token for gateway OMI writes
"""

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


class TestWsopDenial:
    """Deny/timeout all joiner attempts; verify fallback to captive portal."""

    @pytest.fixture(autouse=True)
    def _setup_joiner(self, joiner_port):
        """Erase joiner NVS for each test."""
        erase_nvs(joiner_port)

    def test_explicit_denial_triggers_fallback(self, gateway_url, token,
                                                joiner_port):
        """Deny the joiner's request. Joiner should fall back to captive portal.

        SC-004: Fallback occurs within ~80s (6 retries with denial between each).
        We deny early so this test is faster.
        """
        # Boot joiner — it starts WSOP flow
        reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )

        # Wait for the request to appear on gateway, then deny it
        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        joiner_mac = pending[0]["mac"]
        deny_joiner(gateway_url, joiner_mac, token)

        # The joiner will retry with fresh keypair. Deny each retry.
        # After MAX_RETRY_ATTEMPTS (6), joiner falls back to portal.
        # We monitor serial for the fallback message.
        import serial as pyserial
        ser = pyserial.Serial(joiner_port, 115200, timeout=1)
        fallback = False
        deadline = time.monotonic() + 120  # generous timeout for retries
        denied_count = 1  # already denied one above

        try:
            while time.monotonic() < deadline:
                raw = ser.readline()
                if not raw:
                    # Between serial reads, check for new pending requests to deny
                    if denied_count < 6:
                        try:
                            p = wait_for_pending_request(
                                gateway_url, token, timeout=5
                            )
                            if p and p[0]["mac"] == joiner_mac:
                                deny_joiner(gateway_url, joiner_mac, token)
                                denied_count += 1
                        except TimeoutError:
                            pass
                    continue
                line = raw.decode("utf-8", errors="replace").strip()
                if "falling back to portal" in line or "falling back to captive" in line:
                    fallback = True
                    break
                if "WSOP: exhausted" in line:
                    fallback = True
                    break
        finally:
            ser.close()

        assert fallback, (
            f"Joiner did not fall back to captive portal after denial "
            f"(denied {denied_count} times)"
        )

    def test_timeout_triggers_fallback(self, gateway_url, token, joiner_port):
        """Let the approval window expire without approving or denying.

        The gateway auto-denies after 60s (FR-124). After retries, the joiner
        should fall back to captive portal.
        """
        # Boot joiner
        reboot_and_capture_serial(
            joiner_port,
            ["WSOP: sending JoinRequest"],
            timeout=JOINER_BOOT_TIMEOUT,
        )

        # Verify the request appears on gateway but do NOT approve/deny
        pending = wait_for_pending_request(gateway_url, token, timeout=30)
        assert len(pending) >= 1, "Expected pending join request"

        # Wait for joiner to exhaust retries and fall back
        # The joiner polls every 10s, max 6 poll attempts per request,
        # with up to 6 overall retry attempts. Total timeout can be long.
        import serial as pyserial
        ser = pyserial.Serial(joiner_port, 115200, timeout=1)
        fallback = False
        deadline = time.monotonic() + 180  # generous: gateway timeout + retries

        try:
            while time.monotonic() < deadline:
                raw = ser.readline()
                if not raw:
                    continue
                line = raw.decode("utf-8", errors="replace").strip()
                if "falling back to portal" in line or "falling back to captive" in line:
                    fallback = True
                    break
                if "WSOP: exhausted" in line:
                    fallback = True
                    break
        finally:
            ser.close()

        assert fallback, "Joiner did not fall back to portal after approval timeout"
