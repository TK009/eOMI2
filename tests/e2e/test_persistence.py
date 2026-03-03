"""Section 10 — NVS Persistence.

Verify that user-written data survives a device reboot and that the
sensor tree (System/FreeHeap) is rebuilt from code after restart.
"""

import time
import warnings

import pytest

from helpers import (
    omi_delete,
    omi_read,
    omi_write,
    reboot_device,
    wait_for_device,
    wait_for_device_down,
)

pytestmark = pytest.mark.reboot

# The firmware main loop runs every 5 s and commits dirty NVS data on each
# iteration (see src/main.rs, main loop sleep).  We wait one full interval
# plus a safety margin so the write is guaranteed to be flushed.
NVS_FLUSH_WAIT_S = 7  # 5 s interval + 2 s margin


@pytest.fixture(scope="module")
def rebooted_device(base_url, token, device_port):
    """Write a value, wait for NVS flush, reboot, and wait for recovery."""
    # Write a value that should survive the reboot
    data = omi_write(base_url, "/Persist/Key", "saved", token=token)
    assert data["response"]["status"] in (200, 201)

    # Wait for NVS dirty-flag flush
    time.sleep(NVS_FLUSH_WAIT_S)

    # Hardware reset
    reboot_device(device_port)

    # Wait for the device to actually go offline before polling for recovery,
    # otherwise we might get a stale pre-reboot HTTP 200.
    wait_for_device_down(base_url, timeout=10)

    # Wait for the device to come back online (OMI subsystem ready)
    wait_for_device(base_url, timeout=30)

    yield

    # Cleanup: best-effort delete
    try:
        omi_delete(base_url, "/Persist", token=token)
    except Exception as exc:
        warnings.warn(f"Cleanup of /Persist failed: {exc}")


def test_user_data_survives_reboot(rebooted_device, base_url, token):
    """NVS-persisted value is still readable after a hardware reset."""
    data = omi_read(base_url, "/Persist/Key", token=token, newest=1)
    assert data["response"]["status"] == 200
    values = data["response"]["result"]["values"]
    assert values[0]["v"] == "saved"


def test_sensor_tree_rebuilt(rebooted_device, base_url):
    """Sensor tree is rebuilt from code after reboot (System/FreeHeap exists)."""
    data = omi_read(base_url, "/System")
    assert data["response"]["status"] == 200
    result = data["response"]["result"]
    assert "FreeHeap" in result["items"]
