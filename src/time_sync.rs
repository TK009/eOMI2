//! SNTP time synchronization wrapper.
//!
//! Initializes SNTP in non-blocking mode so the ESP-IDF kernel updates
//! `SystemTime` in the background. No changes needed to `now_secs()` or
//! `Value::now()` — they read `SystemTime` which is automatically updated.
//!
//! On host (non-ESP), provides a stub for lifecycle testing.

#[cfg(feature = "esp")]
mod esp_impl {
    use esp_idf_svc::sntp::{EspSntp, SyncStatus};
    use log::{info, warn};

    /// Holds the SNTP handle. Dropping this stops the SNTP service.
    pub struct TimeSync {
        _sntp: EspSntp<'static>,
    }

    impl TimeSync {
        /// Start SNTP synchronization with default NTP servers (pool.ntp.org).
        ///
        /// This is non-blocking: SNTP syncs in the background via the ESP-IDF
        /// SNTP task. Returns immediately after initialization.
        pub fn start() -> Option<Self> {
            match EspSntp::new_default() {
                Ok(sntp) => {
                    info!("SNTP time sync started (pool.ntp.org)");
                    Some(Self { _sntp: sntp })
                }
                Err(e) => {
                    warn!("Failed to start SNTP time sync: {}", e);
                    None
                }
            }
        }

        /// Check the current synchronization status.
        pub fn sync_status(&self) -> SyncStatus {
            self._sntp.get_sync_status()
        }
    }
}

#[cfg(feature = "esp")]
pub use esp_impl::TimeSync;

// Host stub for testing — no-op implementation.
#[cfg(not(feature = "esp"))]
mod host_stub {
    /// Host-side stub for TimeSync. Records state for lifecycle testing.
    pub struct TimeSync {
        running: bool,
    }

    impl TimeSync {
        /// Start SNTP synchronization (host stub — always succeeds).
        pub fn start() -> Option<Self> {
            Some(Self { running: true })
        }

        /// Whether the SNTP service is running.
        pub fn is_running(&self) -> bool {
            self.running
        }
    }

    impl Drop for TimeSync {
        fn drop(&mut self) {
            self.running = false;
        }
    }
}

#[cfg(not(feature = "esp"))]
pub use host_stub::TimeSync;
