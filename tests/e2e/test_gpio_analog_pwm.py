"""E2E tests for analog_in and PWM GPIO InfoItems (spec 005).

Verify that analog_in pins appear in the O-DF tree with mode='analog_in'
metadata, return numeric ADC values (0..4095), and reject writes.
Verify that PWM pins appear with mode='pwm' metadata, accept duty-cycle
writes (0..255), clamp out-of-range values, and reflect written values.

Requirements: FR-005, FR-006, FR-007
Acceptance: US2 scenarios

Environment variables (override defaults from board config):
  ANALOG_IN_PATH – O-DF path to an analog_in pin  (default: /GPIO34)
  PWM_PATH       – O-DF path to a pwm pin         (default: /GPIO25)
"""

import os
import time

from helpers import omi_read, omi_write, omi_status, omi_result


ANALOG_IN_PATH = os.environ.get("ANALOG_IN_PATH", "/GPIO34")
PWM_PATH = os.environ.get("PWM_PATH", "/GPIO25")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_item_meta(base_url, path):
    """Read an InfoItem and return its metadata dict."""
    data = omi_read(base_url, path=path)
    assert omi_status(data) == 200, f"failed to read {path}: {data}"
    result = omi_result(data)
    return result.get("meta", {})


# ===========================================================================
# analog_in tests
# ===========================================================================

# ---------------------------------------------------------------------------
# FR-005: analog_in InfoItem visible in tree
# ---------------------------------------------------------------------------

def test_analog_in_in_tree(base_url):
    """analog_in InfoItem is present in the O-DF tree (FR-005)."""
    data = omi_read(base_url, path=ANALOG_IN_PATH)
    assert omi_status(data) == 200, (
        f"{ANALOG_IN_PATH} not found in tree"
    )


# ---------------------------------------------------------------------------
# FR-005: analog_in metadata includes mode='analog_in'
# ---------------------------------------------------------------------------

def test_analog_in_metadata_mode(base_url):
    """analog_in InfoItem metadata contains mode='analog_in' (FR-005)."""
    meta = _read_item_meta(base_url, ANALOG_IN_PATH)
    assert "mode" in meta, f"metadata for {ANALOG_IN_PATH} missing 'mode' field"
    assert meta["mode"] == "analog_in"


# ---------------------------------------------------------------------------
# FR-005: analog_in read returns numeric value in ADC range
# ---------------------------------------------------------------------------

def test_analog_in_read_numeric(base_url):
    """Reading analog_in returns a numeric value in 0..4095 (FR-005)."""
    data = omi_read(base_url, ANALOG_IN_PATH, newest=1)
    assert omi_status(data) == 200
    values = omi_result(data)["values"]
    assert len(values) >= 1, f"expected at least one value for {ANALOG_IN_PATH}"
    v = values[0]["v"]
    assert isinstance(v, (int, float)), (
        f"expected numeric value, got {type(v).__name__}: {v}"
    )
    assert 0 <= v <= 4095, f"ADC value {v} outside 12-bit range 0..4095"


def test_analog_in_value_has_timestamp(base_url):
    """analog_in values include a timestamp (FR-007)."""
    data = omi_read(base_url, ANALOG_IN_PATH, newest=1)
    assert omi_status(data) == 200
    values = omi_result(data)["values"]
    assert len(values) >= 1
    assert "t" in values[0], "value entry should include timestamp 't'"


# ---------------------------------------------------------------------------
# FR-006: analog_in rejects writes
# ---------------------------------------------------------------------------

def test_analog_in_write_rejected(base_url, token):
    """Writing to an analog_in pin returns 403 Forbidden (FR-006)."""
    data = omi_write(base_url, ANALOG_IN_PATH, 100, token=token)
    assert omi_status(data) == 403
    assert data["response"].get("desc"), "403 response should include a description"


# ===========================================================================
# PWM tests
# ===========================================================================

# ---------------------------------------------------------------------------
# FR-005: PWM InfoItem visible in tree
# ---------------------------------------------------------------------------

def test_pwm_in_tree(base_url):
    """PWM InfoItem is present in the O-DF tree (FR-005)."""
    data = omi_read(base_url, path=PWM_PATH)
    assert omi_status(data) == 200, (
        f"{PWM_PATH} not found in tree"
    )


# ---------------------------------------------------------------------------
# FR-005: PWM metadata includes mode='pwm'
# ---------------------------------------------------------------------------

def test_pwm_metadata_mode(base_url):
    """PWM InfoItem metadata contains mode='pwm' (FR-005)."""
    meta = _read_item_meta(base_url, PWM_PATH)
    assert "mode" in meta, f"metadata for {PWM_PATH} missing 'mode' field"
    assert meta["mode"] == "pwm"


# ---------------------------------------------------------------------------
# FR-004: PWM accepts duty-cycle writes
# ---------------------------------------------------------------------------

def test_pwm_write_duty(base_url, token):
    """Writing a valid duty cycle (200) to PWM returns success (FR-004)."""
    data = omi_write(base_url, PWM_PATH, 200, token=token)
    assert omi_status(data) in (200, 201)


def test_pwm_write_zero(base_url, token):
    """Writing duty 0 to PWM returns success (FR-004)."""
    data = omi_write(base_url, PWM_PATH, 0, token=token)
    assert omi_status(data) in (200, 201)


def test_pwm_write_max(base_url, token):
    """Writing duty 255 (max) to PWM returns success (FR-004)."""
    data = omi_write(base_url, PWM_PATH, 255, token=token)
    assert omi_status(data) in (200, 201)


def test_pwm_read_back(base_url, token):
    """Value written to PWM can be read back (FR-004, FR-007)."""
    data = omi_write(base_url, PWM_PATH, 200, token=token)
    assert omi_status(data) in (200, 201)

    read = omi_read(base_url, PWM_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 200


# ---------------------------------------------------------------------------
# FR-007: Out-of-range PWM duty is clamped
# ---------------------------------------------------------------------------

def test_pwm_clamp_high(base_url, token):
    """Writing duty > 255 is clamped to 255 (FR-007)."""
    data = omi_write(base_url, PWM_PATH, 999, token=token)
    assert omi_status(data) in (200, 201)

    read = omi_read(base_url, PWM_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 255, (
        f"expected clamped duty 255, got {values[0]['v']}"
    )


def test_pwm_clamp_negative(base_url, token):
    """Writing negative duty is clamped to 0 (FR-007)."""
    data = omi_write(base_url, PWM_PATH, -10, token=token)
    assert omi_status(data) in (200, 201)

    read = omi_read(base_url, PWM_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 0, (
        f"expected clamped duty 0, got {values[0]['v']}"
    )


# ---------------------------------------------------------------------------
# FR-007: PWM value history
# ---------------------------------------------------------------------------

def test_pwm_value_history(base_url, token):
    """Writing multiple duties records all values in history (FR-007)."""
    omi_write(base_url, PWM_PATH, 50, token=token)
    time.sleep(0.1)
    omi_write(base_url, PWM_PATH, 150, token=token)

    read = omi_read(base_url, PWM_PATH, token=token, newest=2)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 2, (
        f"expected at least 2 history values, got {len(values)}"
    )


def test_pwm_value_has_timestamp(base_url, token):
    """PWM values include a timestamp (FR-007)."""
    omi_write(base_url, PWM_PATH, 100, token=token)

    read = omi_read(base_url, PWM_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert "t" in values[0], "value entry should include timestamp 't'"
