// Memory statistics readers for ESP32 and host.
//
// On ESP: queries real hardware via ESP-IDF APIs.
// On host: all readers return None.

/// Memory statistics for a resource (free and total bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemStat {
    pub free: u32,
    pub total: u32,
}

// ---------------------------------------------------------------------------
// ESP implementations
// ---------------------------------------------------------------------------

#[cfg(feature = "esp")]
mod imp {
    use super::MemStat;

    /// Free and total heap (internal SRAM).
    pub fn heap() -> Option<MemStat> {
        unsafe {
            let free = esp_idf_svc::sys::esp_get_free_heap_size();
            let total = esp_idf_svc::sys::esp_get_heap_size();
            Some(MemStat {
                free: free as u32,
                total: total as u32,
            })
        }
    }

    /// Free and total PSRAM (external SPI RAM).
    ///
    /// Returns `None` if PSRAM is not available (total == 0).
    pub fn psram() -> Option<MemStat> {
        unsafe {
            let free = esp_idf_svc::sys::heap_caps_get_free_size(
                esp_idf_svc::sys::MALLOC_CAP_SPIRAM,
            );
            let total = esp_idf_svc::sys::heap_caps_get_total_size(
                esp_idf_svc::sys::MALLOC_CAP_SPIRAM,
            );
            if total == 0 {
                return None;
            }
            Some(MemStat {
                free: free as u32,
                total: total as u32,
            })
        }
    }

    /// NVS usage statistics for the default partition.
    ///
    /// Returns `None` if the query fails.
    pub fn nvs() -> Option<MemStat> {
        let mut stats = esp_idf_svc::sys::nvs_stats_t {
            used_entries: 0,
            free_entries: 0,
            total_entries: 0,
            namespace_count: 0,
        };
        let ret = unsafe {
            esp_idf_svc::sys::nvs_get_stats(core::ptr::null(), &mut stats)
        };
        if ret != esp_idf_svc::sys::ESP_OK {
            return None;
        }
        Some(MemStat {
            free: stats.free_entries as u32,
            total: stats.total_entries as u32,
        })
    }

    /// Free and total bytes on the data partition (FAT/SPIFFS).
    ///
    /// Returns `None` if the partition is not mounted or the query fails.
    pub fn flash() -> Option<MemStat> {
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let base_path = b"/data\0";
        let ret = unsafe {
            esp_idf_svc::sys::esp_vfs_fat_info(
                base_path.as_ptr() as *const core::ffi::c_char,
                &mut total,
                &mut free,
            )
        };
        if ret != esp_idf_svc::sys::ESP_OK {
            return None;
        }
        // Clamp to u32 — partitions on ESP32 are always < 4 GiB.
        Some(MemStat {
            free: free as u32,
            total: total as u32,
        })
    }
}

// ---------------------------------------------------------------------------
// Host stubs
// ---------------------------------------------------------------------------

#[cfg(not(feature = "esp"))]
mod imp {
    use super::MemStat;

    pub fn heap() -> Option<MemStat> {
        None
    }

    pub fn psram() -> Option<MemStat> {
        None
    }

    pub fn nvs() -> Option<MemStat> {
        None
    }

    pub fn flash() -> Option<MemStat> {
        None
    }
}

// ---------------------------------------------------------------------------
// Public API — re-export from the platform module
// ---------------------------------------------------------------------------

pub use imp::{flash, heap, nvs, psram};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stubs_return_none() {
        assert_eq!(heap(), None);
        assert_eq!(psram(), None);
        assert_eq!(nvs(), None);
        assert_eq!(flash(), None);
    }

    #[test]
    fn mem_stat_debug_and_clone() {
        let s = MemStat { free: 100, total: 200 };
        let s2 = s;
        assert_eq!(s, s2);
        assert_eq!(format!("{:?}", s), "MemStat { free: 100, total: 200 }");
    }
}
