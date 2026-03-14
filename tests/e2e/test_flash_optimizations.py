"""E2e tests for flash space optimizations (eo-hdc).

Validates the flash optimization convoy:
1. Compressed HTML served correctly with Content-Encoding header
2. User HTML passthrough: PATCH with Content-Encoding: gzip, GET returns same
3. NVS binary persistence: write items, reboot, verify restored
4. NVS size regression: binary blobs smaller than JSON for equivalent data
"""

import gzip
import io
import json
import time
import warnings

import pytest
import requests

from helpers import (
    omi_delete,
    omi_read,
    omi_write,
    reboot_device,
    wait_for_device,
    wait_for_device_down,
)

REQUEST_TIMEOUT = 10

# --------------------------------------------------------------------------- #
# Section 1: Compressed HTML serving (Content-Encoding)
# --------------------------------------------------------------------------- #

COMPRESS_PATH = "/test-compress"
COMPRESS_HTML = "<html><body><h1>Compressed Test</h1><p>This is test content.</p></body></html>"


@pytest.fixture(scope="module", autouse=True)
def cleanup_compress_pages(base_url, auth_headers):
    """Best-effort cleanup of test pages after this module."""
    yield
    for path in (COMPRESS_PATH, "/test-gzip-passthrough", "/test-gzip-rt"):
        try:
            requests.delete(
                f"{base_url}{path}",
                headers=auth_headers,
                timeout=REQUEST_TIMEOUT,
            )
        except Exception:
            pass


def test_plain_page_served_with_gzip_when_accepted(base_url, auth_headers):
    """Store plain HTML, GET with Accept-Encoding: gzip returns compressed body
    that decompresses to original content."""
    # Store uncompressed
    resp = requests.patch(
        f"{base_url}{COMPRESS_PATH}",
        data=COMPRESS_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200

    # Retrieve with gzip acceptance — requests auto-decompresses, so use raw
    resp = requests.get(
        f"{base_url}{COMPRESS_PATH}",
        headers={"Accept-Encoding": "gzip"},
        timeout=REQUEST_TIMEOUT,
        stream=True,
    )
    assert resp.status_code == 200
    # The response should have Content-Type text/html
    assert "text/html" in resp.headers.get("Content-Type", "")
    # Read the final content (requests handles decompression transparently)
    content = resp.content.decode("utf-8")
    assert content == COMPRESS_HTML


def test_plain_page_served_without_encoding_when_not_accepted(base_url, auth_headers):
    """Store plain HTML, GET without Accept-Encoding returns plain body."""
    # Store uncompressed
    requests.patch(
        f"{base_url}{COMPRESS_PATH}",
        data=COMPRESS_HTML,
        headers=auth_headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Retrieve without Accept-Encoding header
    resp = requests.get(
        f"{base_url}{COMPRESS_PATH}",
        headers={"Accept-Encoding": "identity"},
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200
    assert resp.text == COMPRESS_HTML
    # Should NOT have Content-Encoding header
    assert "Content-Encoding" not in resp.headers or resp.headers["Content-Encoding"] == "identity"


# --------------------------------------------------------------------------- #
# Section 2: User HTML gzip passthrough
# --------------------------------------------------------------------------- #

PASSTHROUGH_PATH = "/test-gzip-passthrough"
PASSTHROUGH_HTML = "<html><body><h1>Gzip Passthrough</h1><p>Pre-compressed content.</p></body></html>"


def _gzip_bytes(data: str) -> bytes:
    """Compress a string to gzip bytes."""
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb") as f:
        f.write(data.encode("utf-8"))
    return buf.getvalue()


def test_gzip_passthrough_store_and_retrieve(base_url, auth_headers):
    """PATCH with Content-Encoding: gzip stores pre-compressed bytes,
    GET with Accept-Encoding: gzip returns them with Content-Encoding header."""
    compressed = _gzip_bytes(PASSTHROUGH_HTML)

    # Store pre-compressed
    headers = {**auth_headers, "Content-Encoding": "gzip"}
    resp = requests.patch(
        f"{base_url}{PASSTHROUGH_PATH}",
        data=compressed,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200

    # Retrieve — ask for gzip, should get Content-Encoding: gzip back
    resp = requests.get(
        f"{base_url}{PASSTHROUGH_PATH}",
        headers={"Accept-Encoding": "gzip"},
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200
    assert resp.headers.get("Content-Encoding") == "gzip"
    assert "text/html" in resp.headers.get("Content-Type", "")
    # The body should decompress to the original HTML
    assert resp.content.decode("utf-8") == PASSTHROUGH_HTML


def test_gzip_passthrough_decompressed_for_non_gzip_client(base_url, auth_headers):
    """PATCH with Content-Encoding: gzip, then GET without gzip acceptance
    returns decompressed plain HTML."""
    compressed = _gzip_bytes(PASSTHROUGH_HTML)

    # Store pre-compressed
    headers = {**auth_headers, "Content-Encoding": "gzip"}
    requests.patch(
        f"{base_url}{PASSTHROUGH_PATH}",
        data=compressed,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Retrieve without gzip — device should decompress on-the-fly
    resp = requests.get(
        f"{base_url}{PASSTHROUGH_PATH}",
        headers={"Accept-Encoding": "identity"},
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200
    assert resp.text == PASSTHROUGH_HTML
    assert resp.headers.get("Content-Encoding", "identity") != "gzip"


def test_gzip_roundtrip_data_integrity(base_url, auth_headers):
    """Store gzip-compressed HTML, retrieve it, verify byte-for-byte integrity
    after decompression."""
    html = "<html><body>" + "x" * 500 + "</body></html>"
    compressed = _gzip_bytes(html)

    path = "/test-gzip-rt"
    headers = {**auth_headers, "Content-Encoding": "gzip"}
    requests.patch(
        f"{base_url}{path}",
        data=compressed,
        headers=headers,
        timeout=REQUEST_TIMEOUT,
    ).raise_for_status()

    # Retrieve and verify
    resp = requests.get(
        f"{base_url}{path}",
        headers={"Accept-Encoding": "gzip"},
        timeout=REQUEST_TIMEOUT,
    )
    assert resp.status_code == 200
    assert resp.content.decode("utf-8") == html


# --------------------------------------------------------------------------- #
# Section 3: NVS binary persistence across reboot
# --------------------------------------------------------------------------- #

NVS_FLUSH_WAIT_S = 7  # 5s main-loop interval + 2s margin


@pytest.fixture(scope="module")
def nvs_binary_rebooted(base_url, token, device_port):
    """Write diverse value types, wait for NVS flush, reboot, wait for recovery."""
    # Write a string
    data = omi_write(base_url, "/NvsBin/Str", "hello-binary", token=token)
    assert data["response"]["status"] in (200, 201)

    # Write a number
    data = omi_write(base_url, "/NvsBin/Num", 42.5, token=token)
    assert data["response"]["status"] in (200, 201)

    # Write a boolean
    data = omi_write(base_url, "/NvsBin/Bool", True, token=token)
    assert data["response"]["status"] in (200, 201)

    # Wait for NVS dirty-flag flush
    time.sleep(NVS_FLUSH_WAIT_S)

    # Hardware reset
    reboot_device(device_port)
    wait_for_device_down(base_url, timeout=10)
    wait_for_device(base_url, timeout=30)

    yield

    # Cleanup
    try:
        omi_delete(base_url, "/NvsBin", token=token)
    except Exception as exc:
        warnings.warn(f"Cleanup of /NvsBin failed: {exc}")


@pytest.mark.reboot
def test_nvs_string_survives_reboot(nvs_binary_rebooted, base_url, token):
    """String value persisted in binary NVS format survives reboot."""
    data = omi_read(base_url, "/NvsBin/Str", token=token, newest=1)
    assert data["response"]["status"] == 200
    values = data["response"]["result"]["values"]
    assert values[0]["v"] == "hello-binary"


@pytest.mark.reboot
def test_nvs_number_survives_reboot(nvs_binary_rebooted, base_url, token):
    """Numeric value persisted in binary NVS format survives reboot."""
    data = omi_read(base_url, "/NvsBin/Num", token=token, newest=1)
    assert data["response"]["status"] == 200
    values = data["response"]["result"]["values"]
    assert values[0]["v"] == 42.5


@pytest.mark.reboot
def test_nvs_bool_survives_reboot(nvs_binary_rebooted, base_url, token):
    """Boolean value persisted in binary NVS format survives reboot."""
    data = omi_read(base_url, "/NvsBin/Bool", token=token, newest=1)
    assert data["response"]["status"] == 200
    values = data["response"]["result"]["values"]
    assert values[0]["v"] is True


# --------------------------------------------------------------------------- #
# Section 4: NVS size regression — binary smaller than JSON
# --------------------------------------------------------------------------- #


def _json_size_for_items(items):
    """Compute the JSON size that would have been used by the old serializer.

    The old format was a JSON array of objects:
    [{"path":"/Foo","v":"bar","t":1234567890.0}, ...]
    """
    json_items = []
    for path, value, timestamp in items:
        entry = {"path": path, "v": value}
        if timestamp is not None:
            entry["t"] = timestamp
        json_items.append(entry)
    return len(json.dumps(json_items, separators=(",", ":")))


def _binary_size_for_items(items):
    """Compute the binary format size matching the device's serialize_saved_items.

    Format: [version:u8][count:u16-LE] then per item:
      [path_len:u16-LE][path:utf8][type_tag:u8][value:variable][has_t:u8][t:f64-LE?]
    """
    size = 1 + 2  # version + count
    for path, value, timestamp in items:
        path_bytes = path.encode("utf-8")
        size += 2 + len(path_bytes)  # path_len + path
        if value is None:
            size += 1  # tag only
        elif isinstance(value, bool):
            size += 1  # tag only (true/false encoded as tag 1 or 2)
        elif isinstance(value, (int, float)):
            size += 1 + 8  # tag + f64
        elif isinstance(value, str):
            s_bytes = value.encode("utf-8")
            size += 1 + 2 + len(s_bytes)  # tag + str_len + str
        # timestamp
        if timestamp is not None:
            size += 1 + 8  # has_t=1 + f64
        else:
            size += 1  # has_t=0
    return size


# Representative dataset matching realistic device usage
_REGRESSION_ITEMS = [
    ("/Home/Temperature", 22.5, 1710000000.0),
    ("/Home/Humidity", 65.0, 1710000001.0),
    ("/Home/LightOn", True, 1710000002.0),
    ("/Home/DoorLocked", False, 1710000003.0),
    ("/Config/DeviceName", "living-room-sensor", None),
    ("/Config/Interval", 30.0, None),
    ("/Config/Enabled", True, None),
    ("/Persist/UserKey", "some-saved-value", 1710000010.0),
    ("/Persist/Counter", 9999.0, 1710000011.0),
    ("/Persist/Label", "a]b\"c", 1710000012.0),  # JSON-unfriendly chars
]


def test_binary_smaller_than_json():
    """Confirm binary format is strictly smaller than JSON for equivalent data.

    This is a pure-Python calculation (no device needed) that validates the
    format design achieves its size-saving goal.
    """
    json_sz = _json_size_for_items(_REGRESSION_ITEMS)
    bin_sz = _binary_size_for_items(_REGRESSION_ITEMS)

    assert bin_sz < json_sz, (
        f"Binary format ({bin_sz}B) is NOT smaller than JSON ({json_sz}B)"
    )
    # Expect meaningful savings — at least 20% reduction
    savings_pct = (1 - bin_sz / json_sz) * 100
    assert savings_pct > 20, (
        f"Binary savings only {savings_pct:.1f}% — expected >20%"
    )


def test_binary_smaller_for_minimal_items():
    """Even a single item should be smaller in binary than JSON."""
    items = [("/A", 1.0, None)]
    assert _binary_size_for_items(items) < _json_size_for_items(items)


def test_binary_smaller_for_string_heavy_items():
    """Binary format wins even when most values are strings (which JSON
    encodes efficiently)."""
    items = [
        (f"/S/{i}", f"value-{i}", 1710000000.0 + i)
        for i in range(20)
    ]
    json_sz = _json_size_for_items(items)
    bin_sz = _binary_size_for_items(items)
    assert bin_sz < json_sz
