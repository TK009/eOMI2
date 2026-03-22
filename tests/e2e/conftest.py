"""Shared fixtures for e2e tests."""

import os
import subprocess
import time

import pytest


# ---------------------------------------------------------------------------
# Fixtures (session-scoped)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def device_port():
    """Device serial port from DEVICE_PORT env var."""
    port = os.environ.get("DEVICE_PORT")
    if not port:
        pytest.skip("DEVICE_PORT not set – reboot tests need serial access")
    return port


@pytest.fixture(scope="session")
def device_ip():
    """Device IP address from the DEVICE_IP env var."""
    ip = os.environ.get("DEVICE_IP")
    if not ip:
        pytest.skip("DEVICE_IP not set")
    return ip


@pytest.fixture(scope="session")
def base_url(device_ip):
    """Root URL for HTTP requests."""
    return f"http://{device_ip}"


@pytest.fixture(scope="session")
def token():
    """API bearer token from the API_TOKEN env var."""
    tok = os.environ.get("API_TOKEN")
    if not tok:
        pytest.skip("API_TOKEN not set")
    return tok


@pytest.fixture(scope="session")
def auth_headers(token):
    """Authorization header dict."""
    return {"Authorization": f"Bearer {token}"}


@pytest.fixture(scope="session")
def ws_url(device_ip):
    """WebSocket URL for OMI."""
    return f"ws://{device_ip}/omi/ws"


@pytest.fixture(scope="session")
def ota_firmware_a_gz():
    """Path to gzip-compressed firmware version A (for restore)."""
    path = os.environ.get("OTA_FIRMWARE_A_GZ")
    if not path:
        pytest.skip("OTA_FIRMWARE_A_GZ not set")
    return path


@pytest.fixture(scope="session")
def ota_firmware_b_gz():
    """Path to gzip-compressed firmware version B (for OTA test)."""
    path = os.environ.get("OTA_FIRMWARE_B_GZ")
    if not path:
        pytest.skip("OTA_FIRMWARE_B_GZ not set")
    return path


@pytest.fixture(scope="session")
def bridge_port():
    """Bridge device serial port from BRIDGE_PORT env var."""
    port = os.environ.get("BRIDGE_PORT")
    if not port:
        pytest.skip("BRIDGE_PORT not set — provisioning tests need a WiFi bridge")
    return port


@pytest.fixture(scope="session")
def bridge(bridge_port):
    """Session-scoped serial bridge to the WiFi bridge ESP32."""
    from serial_bridge import SerialBridge

    b = SerialBridge(bridge_port)
    yield b
    b.close()


# ---------------------------------------------------------------------------
# WSOP two-device fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def gateway_ip(device_ip):
    """Gateway device IP (same as DUT — provisioned device acting as gateway)."""
    return device_ip


@pytest.fixture(scope="session")
def gateway_url(gateway_ip):
    """Root URL for the gateway device."""
    return f"http://{gateway_ip}"


@pytest.fixture(scope="session")
def gateway_port(device_port):
    """Gateway device serial port (same as DUT)."""
    return device_port


@pytest.fixture(scope="session")
def joiner_port():
    """Joiner device serial port from JOINER_PORT env var.

    Falls back to BRIDGE_PORT if JOINER_PORT is not set, since the bridge
    device is repurposed as the joiner for WSOP two-device tests.
    """
    port = os.environ.get("JOINER_PORT") or os.environ.get("BRIDGE_PORT")
    if not port:
        pytest.skip("JOINER_PORT/BRIDGE_PORT not set — WSOP tests need a second device")
    return port


def erase_nvs(serial_port):
    """Erase the NVS partition to force a device into unconfigured (joiner) state."""
    subprocess.run(
        ["espflash", "erase-region", "--port", serial_port, "0x9000", "0x6000"],
        check=True,
        capture_output=True,
        timeout=15,
    )


def reboot_and_capture_serial(serial_port, patterns, timeout=60):
    """Reboot a device via espflash and capture serial output until all patterns
    are found or timeout expires.

    Returns a dict mapping each pattern string to the first line that matched it,
    or None if not found within the timeout.
    """
    subprocess.run(
        ["espflash", "reset", "--port", serial_port],
        check=True,
        capture_output=True,
        timeout=10,
    )

    results = {p: None for p in patterns}
    remaining = set(patterns)
    lines_collected = []

    import serial as pyserial
    ser = pyserial.Serial(serial_port, 115200, timeout=1)
    deadline = time.monotonic() + timeout

    try:
        while remaining and time.monotonic() < deadline:
            raw = ser.readline()
            if not raw:
                continue
            try:
                line = raw.decode("utf-8", errors="replace").strip()
            except Exception:
                continue
            lines_collected.append(line)
            for p in list(remaining):
                if p in line:
                    results[p] = line
                    remaining.discard(p)
    finally:
        ser.close()

    results["_all_lines"] = lines_collected
    return results


# ---------------------------------------------------------------------------
# WSOP gateway helpers
# ---------------------------------------------------------------------------

WSOP_PENDING_REQUESTS_PATH = "/Objects/OnboardingGateway/PendingRequests"
WSOP_APPROVAL_PATH = "/Objects/OnboardingGateway/Approval"


def wait_for_pending_request(gateway_url, token, timeout=60):
    """Poll the gateway's PendingRequests InfoItem until a joiner appears.

    Returns the parsed JSON array of pending requests.
    """
    import json
    from helpers import omi_read

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            data = omi_read(gateway_url, WSOP_PENDING_REQUESTS_PATH, token=token)
            if data.get("response", {}).get("status") == 200:
                result = data["response"]["result"]
                values = result.get("values", [])
                if values:
                    raw = values[0].get("v", "[]")
                    pending = json.loads(raw) if isinstance(raw, str) else raw
                    if pending and len(pending) > 0:
                        return pending
        except (KeyError, json.JSONDecodeError, TypeError):
            pass
        time.sleep(3)
    raise TimeoutError(f"No pending join requests within {timeout}s")


def approve_joiner(gateway_url, mac, token):
    """Write an approval for the given MAC to the gateway's Approval InfoItem."""
    import json
    from helpers import omi_write

    approval_json = json.dumps({"mac": mac, "action": "approve"})
    return omi_write(gateway_url, WSOP_APPROVAL_PATH, approval_json, token=token)


def deny_joiner(gateway_url, mac, token):
    """Write a denial for the given MAC to the gateway's Approval InfoItem."""
    import json
    from helpers import omi_write

    denial_json = json.dumps({"mac": mac, "action": "deny"})
    return omi_write(gateway_url, WSOP_APPROVAL_PATH, denial_json, token=token)
