"""E2E validation of System stat items (mem-stats feature).

Verifies that /System/FreeFlash, /System/FreeOdfStorage, and
/System/Temperature (if present) are readable via OMI, return
plausible numeric values, and expose correct metadata.

Requires firmware built with ``--features mem-stats``.
Memory stat items are polled every 30 s in the main loop, so we use
a longer back-off than the default FreeHeap tests.
"""

import pytest

from helpers import omi_read, omi_result, omi_status, wait_for_values

# Memory stats poll every 30 s — allow enough time for first reading.
MEM_STAT_BACKOFF = [2, 5, 5, 10, 10, 10]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _read_item_meta(base_url, path):
    """Read an InfoItem and return its metadata dict."""
    data = omi_read(base_url, path=path)
    assert omi_status(data) == 200, f"expected 200 for {path}, got {omi_status(data)}"
    result = omi_result(data)
    return result.get("meta", {})


def _item_exists(base_url, path):
    """Return True if *path* resolves to a 200 OMI status."""
    data = omi_read(base_url, path=path)
    return omi_status(data) == 200


# ---------------------------------------------------------------------------
# /System/FreeFlash
# ---------------------------------------------------------------------------

class TestFreeFlash:
    PATH = "/System/FreeFlash"

    def test_exists(self, base_url):
        """FreeFlash item is present in the tree (mem-stats enabled)."""
        assert _item_exists(base_url, self.PATH), (
            f"{self.PATH} not found — was firmware built with --features mem-stats?"
        )

    def test_value_positive(self, base_url):
        """FreeFlash value is a positive number (bytes)."""
        values = wait_for_values(base_url, path=self.PATH, delays=MEM_STAT_BACKOFF)
        v = values[0]["v"]
        assert isinstance(v, (int, float)), f"expected number, got {type(v)}"
        assert v > 0, f"FreeFlash should be > 0, got {v}"

    def test_value_plausible(self, base_url):
        """FreeFlash value is in a plausible range (1 KB – 16 MB)."""
        values = wait_for_values(base_url, path=self.PATH, delays=MEM_STAT_BACKOFF)
        v = values[0]["v"]
        assert 1_000 <= v <= 16_000_000, f"FreeFlash {v} out of plausible range"

    def test_meta_unit(self, base_url):
        """FreeFlash metadata has unit='B'."""
        meta = _read_item_meta(base_url, self.PATH)
        assert meta.get("unit") == "B", f"expected unit='B', got {meta}"

    def test_meta_total_positive(self, base_url):
        """FreeFlash metadata.total is > 0 (set at boot from flash size)."""
        meta = _read_item_meta(base_url, self.PATH)
        total = meta.get("total")
        assert isinstance(total, (int, float)), f"expected numeric total, got {total}"
        assert total > 0, f"metadata.total should be > 0, got {total}"


# ---------------------------------------------------------------------------
# /System/FreeOdfStorage
# ---------------------------------------------------------------------------

class TestFreeOdfStorage:
    PATH = "/System/FreeOdfStorage"

    def test_exists(self, base_url):
        """FreeOdfStorage item is present in the tree (mem-stats enabled)."""
        assert _item_exists(base_url, self.PATH), (
            f"{self.PATH} not found — was firmware built with --features mem-stats?"
        )

    def test_value_positive(self, base_url):
        """FreeOdfStorage value is a positive number (bytes)."""
        values = wait_for_values(base_url, path=self.PATH, delays=MEM_STAT_BACKOFF)
        v = values[0]["v"]
        assert isinstance(v, (int, float)), f"expected number, got {type(v)}"
        assert v > 0, f"FreeOdfStorage should be > 0, got {v}"

    def test_value_plausible(self, base_url):
        """FreeOdfStorage value is in a plausible range (100 B – 1 MB)."""
        values = wait_for_values(base_url, path=self.PATH, delays=MEM_STAT_BACKOFF)
        v = values[0]["v"]
        assert 100 <= v <= 1_000_000, f"FreeOdfStorage {v} out of plausible range"

    def test_meta_unit(self, base_url):
        """FreeOdfStorage metadata has unit='B'."""
        meta = _read_item_meta(base_url, self.PATH)
        assert meta.get("unit") == "B", f"expected unit='B', got {meta}"

    def test_meta_total_positive(self, base_url):
        """FreeOdfStorage metadata.total is > 0 (set at boot from NVS stats)."""
        meta = _read_item_meta(base_url, self.PATH)
        total = meta.get("total")
        assert isinstance(total, (int, float)), f"expected numeric total, got {total}"
        assert total > 0, f"metadata.total should be > 0, got {total}"


# ---------------------------------------------------------------------------
# /System/Temperature (board-dependent — may not be present)
# ---------------------------------------------------------------------------

# Temperature polls every 5 min; use the initial boot reading.
TEMP_BACKOFF = [2, 5, 10, 15, 30, 60, 60, 60, 60, 60]


class TestTemperature:
    PATH = "/System/Temperature"

    @pytest.fixture(autouse=True)
    def _skip_if_absent(self, base_url):
        if not _item_exists(base_url, self.PATH):
            pytest.skip("Temperature item not present (board has no temp sensor)")

    def test_value_numeric(self, base_url):
        """Temperature value is a number."""
        values = wait_for_values(base_url, path=self.PATH, delays=TEMP_BACKOFF)
        v = values[0]["v"]
        assert isinstance(v, (int, float)), f"expected number, got {type(v)}"

    def test_value_plausible(self, base_url):
        """Temperature value is in a plausible range (-10 to 85 °C)."""
        values = wait_for_values(base_url, path=self.PATH, delays=TEMP_BACKOFF)
        v = values[0]["v"]
        assert -10 <= v <= 85, f"Temperature {v} °C out of plausible range"

    def test_meta_unit(self, base_url):
        """Temperature metadata has unit='°C'."""
        meta = _read_item_meta(base_url, self.PATH)
        assert meta.get("unit") == "°C", f"expected unit='°C', got {meta}"
