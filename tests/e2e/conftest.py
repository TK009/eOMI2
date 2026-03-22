"""Shared fixtures for e2e tests.

Device locking is handled here via the HTTP lock server (device_lock
module), so only the devices actually needed by collected tests are
claimed.  If you run a subset that doesn't use the bridge, only one
device is locked.
"""

import os
import re
import subprocess
import time

import pytest
import requests

from device_lock import DeviceLock


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _discover_ip(port: str, timeout: float = 30) -> str:
    """Read serial output from *port* until the Wi-Fi IP line appears."""
    import serial as pyserial

    deadline = time.monotonic() + timeout
    ser = pyserial.Serial(port, 115200, timeout=1)
    try:
        while time.monotonic() < deadline:
            raw = ser.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", errors="replace")
            if "Wi-Fi connected. IP:" in line:
                match = re.search(r"(\d+\.\d+\.\d+\.\d+)", line)
                if match:
                    return match.group(1)
    finally:
        ser.close()
    raise TimeoutError(f"Device on {port} did not report an IP within {timeout}s")


def _health_check(ip: str, timeout: float = 15) -> None:
    """Poll ``http://<ip>/`` until it responds 200."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            resp = requests.get(f"http://{ip}/", timeout=5)
            if resp.status_code == 200:
                return
        except requests.RequestException:
            pass
        time.sleep(1)
    raise TimeoutError(f"Health check failed for {ip} within {timeout}s")


# ---------------------------------------------------------------------------
# DUT fixtures (session-scoped)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def dut_lock():
    """Claim and hold the DUT device for the entire test session.

    If DEVICE_PORT is set, locks that specific device (for manual/pinned
    runs).  Otherwise claims the first available device from the lock
    server.

    The lock is released automatically when the session ends (fixture
    teardown stops the heartbeat and calls DELETE on the server).
    """
    pinned = os.environ.get("DEVICE_PORT")
    lock = DeviceLock.claim(device=pinned, timeout=240)
    yield lock
    lock.release()


@pytest.fixture(scope="session")
def device_port(dut_lock):
    """DUT serial port path.

    The same physical device stays locked for the entire session,
    ensuring firmware flashed to it remains accessible.
    """
    return dut_lock.port


@pytest.fixture(scope="session")
def device_ip(dut_lock):
    """DUT IP address — flashes firmware, reads IP from serial, health-checks.

    Always flashes the claimed device and discovers its IP from serial
    output.  This guarantees the locked device is the one with the
    correct firmware — no stale DEVICE_IP pointing at the wrong device.
    """
    port = dut_lock.port
    firmware = os.environ.get("FIRMWARE_PATH")
    if not firmware:
        pytest.skip("FIRMWARE_PATH not set — cannot flash DUT")

    # Erase NVS partition to remove stale data from previous runs.
    # Without this, accumulated test data makes the tree too large for
    # the HTTP response buffer and full-tree reads hang.
    # We erase only the NVS region (not full flash) so the chip never
    # boots without firmware — a full erase leaves GPIO 18 floating,
    # which the WS2812 RGB LED latches as full white.
    subprocess.run(
        ["espflash", "erase-region", "--port", port, "0x10000", "0x6000"],
        check=True,
        timeout=30,
    )

    # Flash (ESP32-S2 with 2 MB firmware takes ~2 min over USB)
    subprocess.run(
        ["espflash", "flash", "--port", port, firmware],
        check=True,
        timeout=180,
    )

    # Discover IP from serial
    ip = _discover_ip(port, timeout=30)

    # Health check
    _health_check(ip, timeout=15)

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


# ---------------------------------------------------------------------------
# Bridge fixtures (session-scoped, only claimed when tests need them)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def bridge_lock(dut_lock):
    """Claim a second device for the WiFi bridge.

    Only evaluated when a test actually requests bridge_port or bridge.
    The dut_lock dependency ensures we exclude the DUT device.
    """
    try:
        lock = DeviceLock.claim(
            exclude={dut_lock.port},
            timeout=60,
        )
    except (RuntimeError, FileNotFoundError, ConnectionError) as exc:
        pytest.skip(f"No bridge device available: {exc}")
    yield lock
    lock.release()


@pytest.fixture(scope="session")
def bridge_port(bridge_lock):
    """Bridge device serial port — only claimed when a test needs it."""
    return bridge_lock.port


@pytest.fixture(scope="session")
def bridge(bridge_port):
    """Session-scoped serial bridge to the WiFi bridge ESP32.

    Flashes bridge firmware if BRIDGE_FIRMWARE is set, then opens serial.
    """
    firmware = os.environ.get("BRIDGE_FIRMWARE")
    if firmware and os.path.isfile(firmware):
        subprocess.run(
            ["espflash", "flash", "--port", bridge_port, firmware],
            check=True,
            timeout=180,
        )

    from serial_bridge import SerialBridge

    b = SerialBridge(bridge_port)
    yield b
    b.close()
