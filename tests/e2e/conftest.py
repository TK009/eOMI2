"""Shared fixtures for e2e tests."""

import os

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
