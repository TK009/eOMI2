// NVS persistence layer for writable O-DF items.
//
// Stores all writable items as a single JSON blob in the ESP-IDF
// Non-Volatile Storage (NVS) under namespace "omi_tree", key "writable".

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use log::{info, warn};

use crate::device::{deserialize_saved_items, serialize_saved_items, SavedItem};

/// NVS namespace for the OMI object tree.
const NVS_NAMESPACE: &str = "omi_tree";

/// NVS key for the writable items blob.
const NVS_KEY: &str = "writable";

/// Open the NVS namespace for OMI tree persistence.
pub fn open_nvs(partition: EspNvsPartition<NvsDefault>) -> Result<EspNvs<NvsDefault>, esp_idf_svc::sys::EspError> {
    EspNvs::new(partition, NVS_NAMESPACE, true)
}

/// Load writable items from NVS. Returns empty vec on missing or corrupt data.
pub fn load_writable_items(nvs: &EspNvs<NvsDefault>) -> Vec<SavedItem> {
    // Get the blob length first
    let len = match nvs.blob_len(NVS_KEY) {
        Ok(Some(len)) => len,
        Ok(None) => {
            info!("NVS: no saved writable items found");
            return Vec::new();
        }
        Err(e) => {
            warn!("NVS: error checking key '{}': {}", NVS_KEY, e);
            return Vec::new();
        }
    };

    let mut buf = vec![0u8; len];
    match nvs.get_blob(NVS_KEY, &mut buf) {
        Ok(Some(data)) => {
            match deserialize_saved_items(data) {
                Ok(items) => {
                    info!("NVS: loaded {} writable items", items.len());
                    items
                }
                Err(e) => {
                    warn!("NVS: failed to deserialize writable items: {}", e);
                    Vec::new()
                }
            }
        }
        Ok(None) => {
            info!("NVS: no saved writable items found");
            Vec::new()
        }
        Err(e) => {
            warn!("NVS: error reading key '{}': {}", NVS_KEY, e);
            Vec::new()
        }
    }
}

/// Save writable items to NVS as a JSON blob.
pub fn save_writable_items(nvs: &mut EspNvs<NvsDefault>, items: &[SavedItem]) {
    let blob = match serialize_saved_items(items) {
        Ok(b) => b,
        Err(e) => {
            warn!("NVS: skipping write: {:?}", e);
            return;
        }
    };
    match nvs.set_blob(NVS_KEY, &blob) {
        Ok(()) => {
            info!("NVS: saved {} writable items ({} bytes)", items.len(), blob.len());
        }
        Err(e) => {
            warn!("NVS: failed to save writable items: {}", e);
        }
    }
}
