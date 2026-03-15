//! SNTP time synchronization wrapper for ESP-IDF.
//!
//! Initializes SNTP in non-blocking mode so the ESP-IDF kernel updates
//! `SystemTime` in the background. No changes needed to `now_secs()` or
//! `Value::now()` — they read `SystemTime` which is automatically updated.

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
