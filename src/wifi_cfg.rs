// WiFi configuration persistence layer.
//
// Stores WiFi credentials, hostname, and API key hash in a dedicated
// NVS namespace "wifi_cfg" as a single JSON blob under key "config".
// Writes are atomic at the ESP-IDF NVS level (single blob write).

/// Maximum number of WiFi access points (set at build time).
pub const MAX_WIFI_APS: usize = {
    // cfg-env-based const: build.rs emits cargo:rustc-cfg=max_wifi_aps="N"
    // We parse it here. Since const fn can't do string parsing, we use
    // the build.rs approach of emitting a rustc-env and parsing at build time.
    //
    // build.rs sets MAX_WIFI_APS as a rustc-env var; we embed it here.
    const VAL: usize = include!(concat!(env!("OUT_DIR"), "/max_wifi_aps.const"));
    VAL
};

/// Default hostname (set at build time, defaults to "eOMI").
pub const DEFAULT_HOSTNAME: &str = env!("EOMI_HOSTNAME");

/// WiFi configuration stored in NVS.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct WifiConfig {
    /// SSID/password pairs, up to MAX_WIFI_APS entries.
    pub ssids: Vec<(String, String)>,
    /// Device hostname for mDNS and captive portal AP SSID.
    pub hostname: String,
    /// Hashed API key (never stored in plaintext).
    pub api_key_hash: Vec<u8>,
}

impl WifiConfig {
    /// Create a new empty config with the default hostname.
    pub fn new() -> Self {
        Self {
            ssids: Vec::new(),
            hostname: DEFAULT_HOSTNAME.to_string(),
            api_key_hash: Vec::new(),
        }
    }

    /// Add an SSID/password pair, respecting the MAX_WIFI_APS limit.
    /// Returns false if the limit is already reached.
    pub fn add_ssid(&mut self, ssid: String, password: String) -> bool {
        if self.ssids.len() >= MAX_WIFI_APS {
            return false;
        }
        self.ssids.push((ssid, password));
        true
    }
}

impl Default for WifiConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers (platform-independent, json-gated)
// ---------------------------------------------------------------------------

/// Maximum blob size for WiFi config NVS storage.
#[cfg(feature = "json")]
pub const MAX_WIFI_CFG_BLOB: usize = 4000;

#[cfg(feature = "json")]
#[derive(Debug, PartialEq)]
pub enum WifiCfgSaveError {
    TooLarge(usize),
    SerializeFailed,
}

#[cfg(feature = "json")]
pub fn serialize_wifi_config(cfg: &WifiConfig) -> Result<Vec<u8>, WifiCfgSaveError> {
    let blob = serde_json::to_vec(cfg).map_err(|_| WifiCfgSaveError::SerializeFailed)?;
    if blob.len() > MAX_WIFI_CFG_BLOB {
        return Err(WifiCfgSaveError::TooLarge(blob.len()));
    }
    Ok(blob)
}

#[cfg(feature = "json")]
pub fn deserialize_wifi_config(data: &[u8]) -> Result<WifiConfig, String> {
    serde_json::from_slice(data).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// NVS operations (esp-only)
// ---------------------------------------------------------------------------

#[cfg(feature = "esp")]
mod nvs_impl {
    use super::*;
    use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
    use log::{debug, info, warn};

    const NVS_NAMESPACE: &str = "wifi_cfg";
    const NVS_KEY: &str = "config";

    /// Open the NVS namespace for WiFi configuration.
    pub fn open_wifi_cfg_nvs(
        partition: EspNvsPartition<NvsDefault>,
    ) -> Result<EspNvs<NvsDefault>, esp_idf_svc::sys::EspError> {
        EspNvs::new(partition, NVS_NAMESPACE, true)
    }

    /// Load WiFi configuration from NVS. Returns None on missing or corrupt data.
    pub fn load_wifi_config(nvs: &EspNvs<NvsDefault>) -> Option<WifiConfig> {
        let len = match nvs.blob_len(NVS_KEY) {
            Ok(Some(len)) => len,
            Ok(None) => {
                debug!("wifi_cfg NVS: no saved config found");
                return None;
            }
            Err(e) => {
                warn!("wifi_cfg NVS: error checking key '{}': {}", NVS_KEY, e);
                return None;
            }
        };

        let mut buf = vec![0u8; len];
        match nvs.get_blob(NVS_KEY, &mut buf) {
            Ok(Some(data)) => match deserialize_wifi_config(data) {
                Ok(cfg) => {
                    info!(
                        "wifi_cfg NVS: loaded config ({} SSIDs, hostname={})",
                        cfg.ssids.len(),
                        cfg.hostname
                    );
                    Some(cfg)
                }
                Err(e) => {
                    warn!("wifi_cfg NVS: failed to deserialize config: {}", e);
                    None
                }
            },
            Ok(None) => {
                debug!("wifi_cfg NVS: no saved config found");
                None
            }
            Err(e) => {
                warn!("wifi_cfg NVS: error reading key '{}': {}", NVS_KEY, e);
                None
            }
        }
    }

    /// Save WiFi configuration to NVS. Atomic at the NVS blob-write level.
    pub fn save_wifi_config(nvs: &mut EspNvs<NvsDefault>, cfg: &WifiConfig) -> bool {
        let blob = match serialize_wifi_config(cfg) {
            Ok(b) => b,
            Err(WifiCfgSaveError::TooLarge(size)) => {
                warn!(
                    "wifi_cfg NVS: config blob is {} bytes (>{} limit), skipping write",
                    size, MAX_WIFI_CFG_BLOB,
                );
                return false;
            }
            Err(e) => {
                warn!("wifi_cfg NVS: skipping write: {:?}", e);
                return false;
            }
        };
        match nvs.set_blob(NVS_KEY, &blob) {
            Ok(()) => {
                info!("wifi_cfg NVS: saved config ({} bytes)", blob.len());
                true
            }
            Err(e) => {
                warn!("wifi_cfg NVS: failed to save config: {}", e);
                false
            }
        }
    }
}

#[cfg(feature = "esp")]
pub use nvs_impl::{load_wifi_config, open_wifi_cfg_nvs, save_wifi_config};

/// Load WiFi config from NVS, returning default config if not found or on error.
#[cfg(feature = "esp")]
pub fn load_wifi_config_or_default(
    partition: esp_idf_svc::nvs::EspNvsPartition<esp_idf_svc::nvs::NvsDefault>,
) -> WifiConfig {
    match open_wifi_cfg_nvs(partition) {
        Ok(nvs) => load_wifi_config(&nvs).unwrap_or_default(),
        Err(e) => {
            log::warn!("wifi_cfg: failed to open NVS: {}", e);
            WifiConfig::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_config_has_defaults() {
        let cfg = WifiConfig::new();
        assert!(cfg.ssids.is_empty());
        assert_eq!(cfg.hostname, DEFAULT_HOSTNAME);
        assert!(cfg.api_key_hash.is_empty());
    }

    #[test]
    fn add_ssid_respects_limit() {
        let mut cfg = WifiConfig::new();
        for i in 0..MAX_WIFI_APS {
            assert!(cfg.add_ssid(format!("net{}", i), format!("pass{}", i)));
        }
        assert!(!cfg.add_ssid("extra".into(), "pass".into()));
        assert_eq!(cfg.ssids.len(), MAX_WIFI_APS);
    }

    #[cfg(feature = "json")]
    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut cfg = WifiConfig::new();
        cfg.ssids.push(("MyNet".into(), "secret123".into()));
        cfg.hostname = "mydevice".into();
        cfg.api_key_hash = vec![0xDE, 0xAD, 0xBE, 0xEF];

        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg, restored);
    }

    #[cfg(feature = "json")]
    #[test]
    fn serialize_empty_config() {
        let cfg = WifiConfig::new();
        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg, restored);
    }

    #[cfg(feature = "json")]
    #[test]
    fn serialize_too_large() {
        let mut cfg = WifiConfig::new();
        // Fill with large data to exceed blob limit
        cfg.hostname = "x".repeat(5000);
        let err = serialize_wifi_config(&cfg).unwrap_err();
        assert!(matches!(err, WifiCfgSaveError::TooLarge(_)));
    }

    #[cfg(feature = "json")]
    #[test]
    fn deserialize_invalid_json() {
        assert!(deserialize_wifi_config(b"not json").is_err());
    }

    #[cfg(feature = "json")]
    #[test]
    fn deserialize_wrong_structure() {
        assert!(deserialize_wifi_config(b"[]").is_err());
    }

    #[cfg(feature = "json")]
    #[test]
    fn api_key_hash_roundtrip() {
        // Simulate a realistic SHA-256 hash stored in api_key_hash
        let mut cfg = WifiConfig::new();
        cfg.api_key_hash = vec![
            0x6a, 0x09, 0xe6, 0x67, 0xbb, 0x67, 0xae, 0x85,
            0x3c, 0x6e, 0xf3, 0x72, 0xa5, 0x4f, 0xf5, 0x3a,
            0x51, 0x0e, 0x52, 0x7f, 0x9b, 0x05, 0x68, 0x8c,
            0x1f, 0x83, 0xd9, 0xab, 0x5b, 0xe0, 0xcd, 0x19,
        ];
        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg.api_key_hash, restored.api_key_hash);
    }

    #[cfg(feature = "json")]
    #[test]
    fn config_at_max_aps_roundtrip() {
        let mut cfg = WifiConfig::new();
        for i in 0..MAX_WIFI_APS {
            assert!(cfg.add_ssid(format!("Network_{}", i), format!("password_{}", i)));
        }
        cfg.hostname = "my-device".into();
        cfg.api_key_hash = vec![0xAA; 32];
        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg, restored);
        assert_eq!(restored.ssids.len(), MAX_WIFI_APS);
    }

    #[cfg(feature = "json")]
    #[test]
    fn unicode_ssid_password_roundtrip() {
        let mut cfg = WifiConfig::new();
        cfg.ssids.push(("カフェ".into(), "пароль123".into()));
        cfg.ssids.push(("Ñoño".into(), "contraseña".into()));
        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg.ssids, restored.ssids);
    }

    #[cfg(feature = "json")]
    #[test]
    fn empty_hostname_roundtrip() {
        let mut cfg = WifiConfig::new();
        cfg.hostname = String::new();
        cfg.ssids.push(("Net".into(), "pass".into()));
        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(restored.hostname, "");
    }

    #[cfg(feature = "json")]
    #[test]
    fn deserialize_missing_fields() {
        // JSON missing required fields should fail
        assert!(deserialize_wifi_config(b"{}").is_err());
        assert!(deserialize_wifi_config(b"{\"ssids\":[]}").is_err());
    }

    #[cfg(feature = "json")]
    #[test]
    fn multiple_ssids_roundtrip() {
        let mut cfg = WifiConfig::new();
        cfg.ssids.push(("Home".into(), "pass1".into()));
        cfg.ssids.push(("Office".into(), "pass2".into()));
        cfg.ssids.push(("Mobile".into(), "pass3".into()));

        let blob = serialize_wifi_config(&cfg).unwrap();
        let restored = deserialize_wifi_config(&blob).unwrap();
        assert_eq!(cfg.ssids, restored.ssids);
    }
}
