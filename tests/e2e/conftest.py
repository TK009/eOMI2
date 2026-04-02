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


def _health_check(ip: str, timeout: float = 30) -> None:
    """Poll OMI read on ``/`` until the tree is populated.

    A simple HTTP 200 on ``/`` only proves the HTTP server is up, but GPIO
    and sensor initialization may still be in progress, leaving the OMI tree
    empty.  We wait until the OMI root read returns at least one child.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            resp = requests.post(
                f"http://{ip}/omi",
                json={"omi": "1.0", "ttl": 0, "read": {"path": "/"}},
                timeout=5,
            )
            if resp.status_code == 200:
                data = resp.json()
                result = data.get("response", {}).get("result")
                if result and len(result) > 0:
                    return
        except (requests.RequestException, ValueError):
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

    # Locate the partition table CSV next to Cargo.toml (three dirs up from
    # tests/e2e/conftest.py → project root).
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.abspath(__file__))))
    partition_table = os.path.join(project_root, "partitions.csv")

    # Erase the region 0x9000–0xEFFF which covers:
    #   - Default layout: NVS at 0x9000 (clears stale test data)
    #   - Custom layout: otadata at 0xD000 (resets OTA boot selection)
    # NVS at 0x10000 (custom layout) is preserved so Wi-Fi credentials
    # survive — without them the device boots into AP mode.
    subprocess.run(
        ["espflash", "erase-region", "--port", port, "0x9000", "0x6000"],
        check=True,
        timeout=30,
    )

    # Use the custom partition table when the firmware fits in the OTA
    # partition (0x1E0000 = 1,966,080 bytes).  The ELF file size overstates
    # the flash image (includes debug info, symbol tables), so convert to
    # binary first for an accurate check.
    OTA_PARTITION_SIZE = 0x1E0000
    import tempfile as _tmpmod
    _fw_bin = os.path.join(_tmpmod.gettempdir(), "firmware-size-check.bin")
    try:
        subprocess.run(
            ["espflash", "save-image", "--chip", "esp32s2",
             "--format", "esp-idf", firmware, _fw_bin],
            check=True, timeout=30, capture_output=True,
        )
        firmware_size = os.path.getsize(_fw_bin)
    except (subprocess.CalledProcessError, FileNotFoundError):
        # Fallback: use ELF size (conservative — may reject firmware that
        # would actually fit, but never falsely accepts oversized firmware).
        firmware_size = os.path.getsize(firmware)
    finally:
        if os.path.exists(_fw_bin):
            os.unlink(_fw_bin)
    use_custom_pt = os.path.isfile(partition_table) and firmware_size <= OTA_PARTITION_SIZE

    flash_cmd = ["espflash", "flash", "--port", port]
    if use_custom_pt:
        flash_cmd += ["--partition-table", partition_table]
    flash_cmd.append(firmware)
    subprocess.run(flash_cmd, check=True, timeout=180)

    # espflash always overwrites the partition table at 0x8000 with a
    # default (factory-only) layout, even when --partition-table is given.
    # Write the custom partition table binary explicitly so the firmware
    # can find OTA partitions at runtime.
    if use_custom_pt:
        import tempfile
        pt_bin = os.path.join(tempfile.gettempdir(), "partitions.bin")
        subprocess.run(
            ["espflash", "partition-table", partition_table,
             "--to-binary", "-o", pt_bin],
            check=True,
            timeout=10,
        )
        subprocess.run(
            ["espflash", "write-bin", "--port", port, "0x8000", pt_bin],
            check=True,
            timeout=30,
        )

    # Discover IP from serial
    ip = _discover_ip(port, timeout=30)

    # Health check
    _health_check(ip, timeout=60)

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
def ota_firmware_a():
    """Path to raw (uncompressed) firmware version A (for restore)."""
    path = os.environ.get("OTA_FIRMWARE_A")
    if not path:
        pytest.skip("OTA_FIRMWARE_A not set")
    return path


@pytest.fixture(scope="session")
def ota_firmware_b():
    """Path to raw (uncompressed) firmware version B (for OTA test)."""
    path = os.environ.get("OTA_FIRMWARE_B")
    if not path:
        pytest.skip("OTA_FIRMWARE_B not set")
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
        bridge_pt = os.path.join(
            os.path.dirname(os.path.abspath(firmware)), "..", "..", "partitions.csv"
        )
        bridge_cmd = ["espflash", "flash", "--port", bridge_port]
        if os.path.isfile(bridge_pt):
            bridge_cmd += ["--partition-table", bridge_pt]
        bridge_cmd.append(firmware)
        subprocess.run(
            bridge_cmd,
            check=True,
            timeout=180,
        )

    from serial_bridge import SerialBridge

    b = SerialBridge(bridge_port)
    yield b
    b.close()
