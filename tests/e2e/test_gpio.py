"""E2E tests for digital GPIO InfoItems (spec 005).

Verify that GPIO pins configured at build time appear in the O-DF tree
with correct metadata, that digital_out pins accept writes, and that
digital_in pins reject writes but return readable numeric values.

Requirements: FR-002, FR-003, FR-004, FR-006
Success criteria: SC-001, SC-007

Environment variables (override defaults from board config):
  GPIO_OUT_PATH  – O-DF path to a digital_out pin (default: /GPIO2)
  GPIO_IN_PATH   – O-DF path to a digital_in pin  (default: /GPIO5)
"""

import os
import time

from helpers import omi_read, omi_write, omi_status, omi_result


GPIO_OUT_PATH = os.environ.get("GPIO_OUT_PATH", "/GPIO2")
GPIO_IN_PATH = os.environ.get("GPIO_IN_PATH", "/GPIO5")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_item_meta(base_url, path):
    """Read an InfoItem and return its metadata dict."""
    data = omi_read(base_url, path=path)
    assert omi_status(data) == 200, f"failed to read {path}: {data}"
    result = omi_result(data)
    return result.get("meta", {})


# ---------------------------------------------------------------------------
# FR-002: GPIO InfoItems visible in tree after boot
# ---------------------------------------------------------------------------

def test_gpio_out_in_tree(base_url):
    """digital_out GPIO InfoItem is present in the O-DF tree (FR-002)."""
    name = GPIO_OUT_PATH.lstrip("/")
    data = omi_read(base_url, path=GPIO_OUT_PATH)
    assert omi_status(data) == 200, (
        f"{GPIO_OUT_PATH} not found in tree — expected InfoItem '{name}'"
    )


def test_gpio_in_in_tree(base_url):
    """digital_in GPIO InfoItem is present in the O-DF tree (FR-002)."""
    name = GPIO_IN_PATH.lstrip("/")
    data = omi_read(base_url, path=GPIO_IN_PATH)
    assert omi_status(data) == 200, (
        f"{GPIO_IN_PATH} not found in tree — expected InfoItem '{name}'"
    )


# ---------------------------------------------------------------------------
# FR-003: GPIO metadata includes mode
# ---------------------------------------------------------------------------

def test_gpio_out_metadata_mode(base_url):
    """digital_out InfoItem metadata contains mode='digital_out' (FR-003)."""
    meta = _read_item_meta(base_url, GPIO_OUT_PATH)
    assert "mode" in meta, f"metadata for {GPIO_OUT_PATH} missing 'mode' field"
    assert meta["mode"] == "digital_out"


def test_gpio_in_metadata_mode(base_url):
    """digital_in InfoItem metadata contains mode='digital_in' (FR-003)."""
    meta = _read_item_meta(base_url, GPIO_IN_PATH)
    assert "mode" in meta, f"metadata for {GPIO_IN_PATH} missing 'mode' field"
    assert meta["mode"] == "digital_in"


# ---------------------------------------------------------------------------
# FR-004: digital_out accepts writes
# ---------------------------------------------------------------------------

def test_digital_out_write_high(base_url, token):
    """Writing 1 to a digital_out pin returns 200 (FR-004)."""
    data = omi_write(base_url, GPIO_OUT_PATH, 1, token=token)
    assert omi_status(data) in (200, 201)


def test_digital_out_write_low(base_url, token):
    """Writing 0 to a digital_out pin returns 200 (FR-004)."""
    data = omi_write(base_url, GPIO_OUT_PATH, 0, token=token)
    assert omi_status(data) in (200, 201)


def test_digital_out_read_back(base_url, token):
    """Value written to digital_out can be read back (FR-004, SC-007)."""
    data = omi_write(base_url, GPIO_OUT_PATH, 1, token=token)
    assert omi_status(data) in (200, 201)

    read = omi_read(base_url, GPIO_OUT_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] in (1, True)


# ---------------------------------------------------------------------------
# FR-006: digital_in rejects writes
# ---------------------------------------------------------------------------

def test_digital_in_write_rejected(base_url, token):
    """Writing to a digital_in pin returns 403 Forbidden (FR-006)."""
    data = omi_write(base_url, GPIO_IN_PATH, 1, token=token)
    assert omi_status(data) == 403
    assert data["response"].get("desc"), "403 response should include a description"


# ---------------------------------------------------------------------------
# FR-005: digital_in readable with numeric value
# ---------------------------------------------------------------------------

def test_digital_in_read_value(base_url):
    """Reading a digital_in pin returns a numeric value (FR-005)."""
    data = omi_read(base_url, GPIO_IN_PATH, newest=1)
    assert omi_status(data) == 200
    values = omi_result(data)["values"]
    assert len(values) >= 1, f"expected at least one value for {GPIO_IN_PATH}"
    v = values[0]["v"]
    assert isinstance(v, (int, float)), f"expected numeric value, got {type(v).__name__}: {v}"


# ---------------------------------------------------------------------------
# SC-007: Value history works for GPIO InfoItems
# ---------------------------------------------------------------------------

def test_digital_out_value_history(base_url, token):
    """Writing twice to digital_out records both values in history (SC-007)."""
    omi_write(base_url, GPIO_OUT_PATH, 0, token=token)
    time.sleep(0.1)
    omi_write(base_url, GPIO_OUT_PATH, 1, token=token)

    read = omi_read(base_url, GPIO_OUT_PATH, token=token, newest=2)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 2, (
        f"expected at least 2 history values, got {len(values)}"
    )


def test_digital_in_value_has_timestamp(base_url):
    """digital_in values include a timestamp (SC-007)."""
    data = omi_read(base_url, GPIO_IN_PATH, newest=1)
    assert omi_status(data) == 200
    values = omi_result(data)["values"]
    assert len(values) >= 1
    assert "t" in values[0], "value entry should include timestamp 't'"
