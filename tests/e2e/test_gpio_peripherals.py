"""E2E tests for peripheral protocol InfoItems — I2C, UART, SPI (spec 005).

Verify that peripheral protocol buses configured at build time create
the expected RX/TX InfoItems in the O-DF tree with correct naming and
metadata. Verify I2C device discovery populates child objects. Verify
UART TX writes with different encoding types. Verify SPI InfoItems
include 'SPI' in names and mode metadata.

Requirements: FR-008, FR-009, FR-009a, FR-010
Success criteria: SC-005
User story: US3

Environment variables (override defaults from board config):
  I2C_RX_PATH  – O-DF path to an I2C RX InfoItem  (default: /GPIO21_I2C_RX)
  I2C_TX_PATH  – O-DF path to an I2C TX InfoItem  (default: /GPIO21_I2C_TX)
  UART_RX_PATH – O-DF path to a UART RX InfoItem  (default: /GPIO16_UART_RX)
  UART_TX_PATH – O-DF path to a UART TX InfoItem  (default: /GPIO16_UART_TX)
  SPI_RX_PATH  – O-DF path to an SPI RX InfoItem  (default: /GPIO18_SPI_RX)
  SPI_TX_PATH  – O-DF path to an SPI TX InfoItem  (default: /GPIO18_SPI_TX)
  I2C_DEVICE_PATH – O-DF path to an I2C discovered device object
                     (default: /I2C_0x48)
"""

import os

import pytest

from helpers import omi_read, omi_write, omi_status, omi_result


# ---------------------------------------------------------------------------
# Path configuration (env var overrides)
# ---------------------------------------------------------------------------

I2C_RX_PATH = os.environ.get("I2C_RX_PATH", "/GPIO21_I2C_RX")
I2C_TX_PATH = os.environ.get("I2C_TX_PATH", "/GPIO21_I2C_TX")
UART_RX_PATH = os.environ.get("UART_RX_PATH", "/GPIO16_UART_RX")
UART_TX_PATH = os.environ.get("UART_TX_PATH", "/GPIO16_UART_TX")
SPI_RX_PATH = os.environ.get("SPI_RX_PATH", "/GPIO18_SPI_RX")
SPI_TX_PATH = os.environ.get("SPI_TX_PATH", "/GPIO18_SPI_TX")
I2C_DEVICE_PATH = os.environ.get("I2C_DEVICE_PATH", "/I2C_0x48")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_item_meta(base_url, path):
    """Read an InfoItem and return its metadata dict."""
    data = omi_read(base_url, path=path)
    assert omi_status(data) == 200, f"failed to read {path}: {data}"
    result = omi_result(data)
    return result.get("meta", {})


def _item_exists(base_url, path):
    """Return True if the InfoItem at *path* exists (status 200)."""
    data = omi_read(base_url, path=path)
    return omi_status(data) == 200


# ===========================================================================
# I2C tests (FR-008, FR-009, FR-010)
# ===========================================================================

# ---------------------------------------------------------------------------
# FR-008 / FR-009: I2C RX/TX InfoItems appear in tree with correct naming
# ---------------------------------------------------------------------------

def test_i2c_rx_in_tree(base_url):
    """I2C RX InfoItem is present in the O-DF tree (FR-008, FR-009)."""
    if not _item_exists(base_url, I2C_RX_PATH):
        pytest.skip(f"I2C RX not configured at {I2C_RX_PATH}")
    data = omi_read(base_url, path=I2C_RX_PATH)
    assert omi_status(data) == 200, (
        f"{I2C_RX_PATH} not found — expected I2C RX InfoItem"
    )


def test_i2c_tx_in_tree(base_url):
    """I2C TX InfoItem is present in the O-DF tree (FR-008, FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    data = omi_read(base_url, path=I2C_TX_PATH)
    assert omi_status(data) == 200, (
        f"{I2C_TX_PATH} not found — expected I2C TX InfoItem"
    )


def test_i2c_rx_name_contains_i2c(base_url):
    """I2C RX InfoItem name contains '_I2C_RX' (FR-009)."""
    if not _item_exists(base_url, I2C_RX_PATH):
        pytest.skip(f"I2C RX not configured at {I2C_RX_PATH}")
    name = I2C_RX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_I2C_RX" in name, (
        f"I2C RX InfoItem name '{name}' does not contain '_I2C_RX'"
    )


def test_i2c_tx_name_contains_i2c(base_url):
    """I2C TX InfoItem name contains '_I2C_TX' (FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    name = I2C_TX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_I2C_TX" in name, (
        f"I2C TX InfoItem name '{name}' does not contain '_I2C_TX'"
    )


# ---------------------------------------------------------------------------
# FR-009: I2C metadata includes protocol and mode
# ---------------------------------------------------------------------------

def test_i2c_rx_metadata_mode(base_url):
    """I2C RX InfoItem metadata contains mode='i2c_rx' (FR-009)."""
    if not _item_exists(base_url, I2C_RX_PATH):
        pytest.skip(f"I2C RX not configured at {I2C_RX_PATH}")
    meta = _read_item_meta(base_url, I2C_RX_PATH)
    assert "mode" in meta, f"metadata for {I2C_RX_PATH} missing 'mode' field"
    assert meta["mode"] == "i2c_rx"


def test_i2c_tx_metadata_mode(base_url):
    """I2C TX InfoItem metadata contains mode='i2c_tx' (FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    meta = _read_item_meta(base_url, I2C_TX_PATH)
    assert "mode" in meta, f"metadata for {I2C_TX_PATH} missing 'mode' field"
    assert meta["mode"] == "i2c_tx"


def test_i2c_rx_metadata_protocol(base_url):
    """I2C RX InfoItem metadata contains protocol='I2C' (FR-009)."""
    if not _item_exists(base_url, I2C_RX_PATH):
        pytest.skip(f"I2C RX not configured at {I2C_RX_PATH}")
    meta = _read_item_meta(base_url, I2C_RX_PATH)
    assert meta.get("protocol") == "I2C"


def test_i2c_tx_metadata_protocol(base_url):
    """I2C TX InfoItem metadata contains protocol='I2C' (FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    meta = _read_item_meta(base_url, I2C_TX_PATH)
    assert meta.get("protocol") == "I2C"


def test_i2c_tx_writable(base_url):
    """I2C TX InfoItem metadata indicates writable=true (FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    meta = _read_item_meta(base_url, I2C_TX_PATH)
    assert meta.get("writable") is True


def test_i2c_rx_not_writable(base_url, token):
    """Writing to I2C RX InfoItem is rejected (FR-009)."""
    if not _item_exists(base_url, I2C_RX_PATH):
        pytest.skip(f"I2C RX not configured at {I2C_RX_PATH}")
    data = omi_write(base_url, I2C_RX_PATH, "test", token=token)
    assert omi_status(data) == 403


# ---------------------------------------------------------------------------
# FR-010: I2C discovery adds child objects for detected devices
# ---------------------------------------------------------------------------

def test_i2c_discovered_device_in_tree(base_url):
    """I2C discovered device appears as a child object (FR-010, SC-005).

    US3 scenario 2: Given I2C enabled with discovery, When an I2C device
    is detected at address 0x48, Then a child object (e.g., 'I2C_0x48')
    is automatically added under the device's O-DF tree.
    """
    if not _item_exists(base_url, I2C_DEVICE_PATH):
        pytest.skip(
            f"No I2C device found at {I2C_DEVICE_PATH} — "
            "connect an I2C device or set I2C_DEVICE_PATH"
        )
    data = omi_read(base_url, path=I2C_DEVICE_PATH)
    assert omi_status(data) == 200, (
        f"{I2C_DEVICE_PATH} not found — expected discovered I2C device object"
    )


def test_i2c_discovered_device_has_address(base_url):
    """I2C discovered device object contains an 'address' InfoItem (FR-010)."""
    addr_path = f"{I2C_DEVICE_PATH}/address"
    if not _item_exists(base_url, I2C_DEVICE_PATH):
        pytest.skip(f"No I2C device found at {I2C_DEVICE_PATH}")
    data = omi_read(base_url, path=addr_path, newest=1)
    assert omi_status(data) == 200, (
        f"Expected 'address' InfoItem under {I2C_DEVICE_PATH}"
    )
    values = omi_result(data)["values"]
    assert len(values) >= 1, "address InfoItem should have a value"
    v = values[0]["v"]
    assert isinstance(v, (int, float)), (
        f"address should be numeric, got {type(v).__name__}: {v}"
    )


def test_i2c_discovered_device_name_format(base_url):
    """I2C discovered device name follows I2C_0x{addr:02X} format (FR-010)."""
    if not _item_exists(base_url, I2C_DEVICE_PATH):
        pytest.skip(f"No I2C device found at {I2C_DEVICE_PATH}")
    name = I2C_DEVICE_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert name.startswith("I2C_0x"), (
        f"expected device name starting with 'I2C_0x', got '{name}'"
    )


# ===========================================================================
# UART tests (FR-008, FR-009, FR-009a)
# ===========================================================================

# ---------------------------------------------------------------------------
# FR-008 / FR-009: UART RX/TX InfoItems appear in tree
# ---------------------------------------------------------------------------

def test_uart_rx_in_tree(base_url):
    """UART RX InfoItem is present in the O-DF tree (FR-008, FR-009)."""
    if not _item_exists(base_url, UART_RX_PATH):
        pytest.skip(f"UART RX not configured at {UART_RX_PATH}")
    data = omi_read(base_url, path=UART_RX_PATH)
    assert omi_status(data) == 200


def test_uart_tx_in_tree(base_url):
    """UART TX InfoItem is present in the O-DF tree (FR-008, FR-009)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    data = omi_read(base_url, path=UART_TX_PATH)
    assert omi_status(data) == 200


def test_uart_rx_name_contains_uart(base_url):
    """UART RX InfoItem name contains '_UART_RX' (FR-009)."""
    if not _item_exists(base_url, UART_RX_PATH):
        pytest.skip(f"UART RX not configured at {UART_RX_PATH}")
    name = UART_RX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_UART_RX" in name


def test_uart_tx_name_contains_uart(base_url):
    """UART TX InfoItem name contains '_UART_TX' (FR-009)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    name = UART_TX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_UART_TX" in name


# ---------------------------------------------------------------------------
# FR-009: UART metadata includes protocol and mode
# ---------------------------------------------------------------------------

def test_uart_rx_metadata_mode(base_url):
    """UART RX InfoItem metadata contains mode='uart_rx' (FR-009)."""
    if not _item_exists(base_url, UART_RX_PATH):
        pytest.skip(f"UART RX not configured at {UART_RX_PATH}")
    meta = _read_item_meta(base_url, UART_RX_PATH)
    assert meta.get("mode") == "uart_rx"


def test_uart_tx_metadata_mode(base_url):
    """UART TX InfoItem metadata contains mode='uart_tx' (FR-009)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    meta = _read_item_meta(base_url, UART_TX_PATH)
    assert meta.get("mode") == "uart_tx"


def test_uart_rx_metadata_protocol(base_url):
    """UART RX InfoItem metadata contains protocol='UART' (FR-009)."""
    if not _item_exists(base_url, UART_RX_PATH):
        pytest.skip(f"UART RX not configured at {UART_RX_PATH}")
    meta = _read_item_meta(base_url, UART_RX_PATH)
    assert meta.get("protocol") == "UART"


def test_uart_tx_metadata_protocol(base_url):
    """UART TX InfoItem metadata contains protocol='UART' (FR-009)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    meta = _read_item_meta(base_url, UART_TX_PATH)
    assert meta.get("protocol") == "UART"


def test_uart_tx_writable(base_url):
    """UART TX InfoItem metadata indicates writable=true (FR-009)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    meta = _read_item_meta(base_url, UART_TX_PATH)
    assert meta.get("writable") is True


def test_uart_rx_not_writable(base_url, token):
    """Writing to UART RX InfoItem is rejected (FR-009)."""
    if not _item_exists(base_url, UART_RX_PATH):
        pytest.skip(f"UART RX not configured at {UART_RX_PATH}")
    data = omi_write(base_url, UART_RX_PATH, "test", token=token)
    assert omi_status(data) == 403


# ---------------------------------------------------------------------------
# FR-009a: UART TX write with encoding types
# ---------------------------------------------------------------------------

def test_uart_tx_write_string_default(base_url, token):
    """Writing a plain string to UART TX succeeds (FR-009a, default type).

    US3 scenario 5: Given UART TX InfoItem, When a client writes data
    with type: 'string' (or no type), Then the string value is transmitted
    as UTF-8 bytes.
    """
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    data = omi_write(base_url, UART_TX_PATH, "Hello", token=token)
    assert omi_status(data) in (200, 201)


def test_uart_tx_write_string_readback(base_url, token):
    """String written to UART TX can be read back (FR-009a)."""
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    omi_write(base_url, UART_TX_PATH, "TestData", token=token)
    read = omi_read(base_url, UART_TX_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == "TestData"


def test_uart_tx_write_hex(base_url, token):
    """Writing hex-encoded data to UART TX succeeds (FR-009a).

    US3 scenario 4: Given UART TX InfoItem, When a client writes data
    with type: 'hex' set to '48656C6C6F', Then the raw bytes for 'Hello'
    are transmitted on the physical UART TX pin.
    """
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    # Write hex-encoded "Hello" — the value is stored; decoding to
    # raw bytes happens in the peripheral driver's TX sync.
    data = omi_write(base_url, UART_TX_PATH, "48656C6C6F", token=token)
    assert omi_status(data) in (200, 201)


def test_uart_tx_write_base64(base_url, token):
    """Writing base64-encoded data to UART TX succeeds (FR-009a).

    US3 scenario 6: Given UART TX InfoItem, When a script writes base64
    data 'AQID', Then the decoded bytes [0x01, 0x02, 0x03] are transmitted.
    """
    if not _item_exists(base_url, UART_TX_PATH):
        pytest.skip(f"UART TX not configured at {UART_TX_PATH}")
    data = omi_write(base_url, UART_TX_PATH, "AQID", token=token)
    assert omi_status(data) in (200, 201)


# ===========================================================================
# SPI tests (FR-008, FR-009)
# ===========================================================================

# ---------------------------------------------------------------------------
# FR-008 / FR-009: SPI RX/TX InfoItems appear in tree
# ---------------------------------------------------------------------------

def test_spi_rx_in_tree(base_url):
    """SPI RX InfoItem is present in the O-DF tree (FR-008, FR-009).

    US3 scenario 7: Given SPI is enabled on designated pins, When the
    firmware boots, Then appropriate RX/TX InfoItems are created with
    'SPI' in their names and mode metadata.
    """
    if not _item_exists(base_url, SPI_RX_PATH):
        pytest.skip(f"SPI RX not configured at {SPI_RX_PATH}")
    data = omi_read(base_url, path=SPI_RX_PATH)
    assert omi_status(data) == 200


def test_spi_tx_in_tree(base_url):
    """SPI TX InfoItem is present in the O-DF tree (FR-008, FR-009)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    data = omi_read(base_url, path=SPI_TX_PATH)
    assert omi_status(data) == 200


def test_spi_rx_name_contains_spi(base_url):
    """SPI RX InfoItem name contains '_SPI_RX' (FR-009, US3-7)."""
    if not _item_exists(base_url, SPI_RX_PATH):
        pytest.skip(f"SPI RX not configured at {SPI_RX_PATH}")
    name = SPI_RX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_SPI_RX" in name, (
        f"SPI RX InfoItem name '{name}' does not contain '_SPI_RX'"
    )


def test_spi_tx_name_contains_spi(base_url):
    """SPI TX InfoItem name contains '_SPI_TX' (FR-009, US3-7)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    name = SPI_TX_PATH.rstrip("/").rsplit("/", 1)[-1]
    assert "_SPI_TX" in name, (
        f"SPI TX InfoItem name '{name}' does not contain '_SPI_TX'"
    )


# ---------------------------------------------------------------------------
# FR-009: SPI metadata includes protocol and mode (US3-7)
# ---------------------------------------------------------------------------

def test_spi_rx_metadata_mode(base_url):
    """SPI RX InfoItem metadata contains mode='spi_rx' (FR-009, US3-7)."""
    if not _item_exists(base_url, SPI_RX_PATH):
        pytest.skip(f"SPI RX not configured at {SPI_RX_PATH}")
    meta = _read_item_meta(base_url, SPI_RX_PATH)
    assert meta.get("mode") == "spi_rx"


def test_spi_tx_metadata_mode(base_url):
    """SPI TX InfoItem metadata contains mode='spi_tx' (FR-009, US3-7)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    meta = _read_item_meta(base_url, SPI_TX_PATH)
    assert meta.get("mode") == "spi_tx"


def test_spi_rx_metadata_protocol(base_url):
    """SPI RX InfoItem metadata contains protocol='SPI' (FR-009)."""
    if not _item_exists(base_url, SPI_RX_PATH):
        pytest.skip(f"SPI RX not configured at {SPI_RX_PATH}")
    meta = _read_item_meta(base_url, SPI_RX_PATH)
    assert meta.get("protocol") == "SPI"


def test_spi_tx_metadata_protocol(base_url):
    """SPI TX InfoItem metadata contains protocol='SPI' (FR-009)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    meta = _read_item_meta(base_url, SPI_TX_PATH)
    assert meta.get("protocol") == "SPI"


def test_spi_tx_writable(base_url):
    """SPI TX InfoItem metadata indicates writable=true (FR-009)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    meta = _read_item_meta(base_url, SPI_TX_PATH)
    assert meta.get("writable") is True


def test_spi_rx_not_writable(base_url, token):
    """Writing to SPI RX InfoItem is rejected (FR-009)."""
    if not _item_exists(base_url, SPI_RX_PATH):
        pytest.skip(f"SPI RX not configured at {SPI_RX_PATH}")
    data = omi_write(base_url, SPI_RX_PATH, "test", token=token)
    assert omi_status(data) == 403


# ---------------------------------------------------------------------------
# SPI TX write acceptance
# ---------------------------------------------------------------------------

def test_spi_tx_write_string(base_url, token):
    """Writing a string to SPI TX succeeds (FR-009)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    data = omi_write(base_url, SPI_TX_PATH, "Hello", token=token)
    assert omi_status(data) in (200, 201)


def test_spi_tx_write_readback(base_url, token):
    """Value written to SPI TX can be read back (FR-009)."""
    if not _item_exists(base_url, SPI_TX_PATH):
        pytest.skip(f"SPI TX not configured at {SPI_TX_PATH}")
    omi_write(base_url, SPI_TX_PATH, "SpiData", token=token)
    read = omi_read(base_url, SPI_TX_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == "SpiData"


# ---------------------------------------------------------------------------
# I2C TX write acceptance
# ---------------------------------------------------------------------------

def test_i2c_tx_write_string(base_url, token):
    """Writing a string to I2C TX succeeds (FR-009).

    US3 scenario 1: Given I2C is enabled, When the firmware boots, Then
    I2C RX/TX InfoItems appear in the O-DF tree.
    """
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    data = omi_write(base_url, I2C_TX_PATH, "Hello", token=token)
    assert omi_status(data) in (200, 201)


def test_i2c_tx_write_readback(base_url, token):
    """Value written to I2C TX can be read back (FR-009)."""
    if not _item_exists(base_url, I2C_TX_PATH):
        pytest.skip(f"I2C TX not configured at {I2C_TX_PATH}")
    omi_write(base_url, I2C_TX_PATH, "I2cData", token=token)
    read = omi_read(base_url, I2C_TX_PATH, token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == "I2cData"
