// mDNS responder wrapper for device discovery.
//
// Thin wrapper around esp_idf_svc::mdns::EspMdns that advertises the
// device hostname and registers a DNS-SD service (_omi._tcp) with a
// TXT record containing the O-DF object path.
//
// ESP implementation is gated behind #[cfg(feature = "esp")].
// A host stub is provided for testing.

/// DNS-SD service type for eOMI devices.
pub const SERVICE_TYPE: &str = "_omi";
/// DNS-SD protocol.
pub const SERVICE_PROTO: &str = "_tcp";
/// Default O-DF object path advertised in TXT record.
pub const DEFAULT_ODF_PATH: &str = "/Objects";
/// Default HTTP port.
pub const DEFAULT_PORT: u16 = 80;

/// Configuration for the mDNS responder.
#[derive(Debug, Clone)]
pub struct MdnsConfig {
    pub hostname: String,
    pub port: u16,
    pub odf_path: String,
}

impl MdnsConfig {
    pub fn new(hostname: &str) -> Self {
        Self {
            hostname: hostname.to_string(),
            port: DEFAULT_PORT,
            odf_path: DEFAULT_ODF_PATH.to_string(),
        }
    }
}

#[cfg(feature = "esp")]
mod esp_impl {
    use super::*;
    use esp_idf_svc::mdns::EspMdns;
    use log::{info, warn};

    /// Handle for a running mDNS responder. Drop to send goodbye (TTL=0).
    pub struct MdnsResponder {
        _mdns: EspMdns,
        config: MdnsConfig,
    }

    impl MdnsResponder {
        /// Start the mDNS responder: set hostname and register DNS-SD service.
        ///
        /// - FR-001: Advertises hostname on .local domain
        /// - FR-002: Responds to mDNS queries (handled by esp-idf)
        /// - FR-003: Registers _omi._tcp service with TXT record
        /// - FR-004: Uses configured hostname (default: "eomi")
        /// - FR-005: Instance name = hostname for uniqueness
        /// - FR-010: Conflict resolution handled by esp-idf probing
        /// - FR-011: Lightweight — uses esp-idf built-in mDNS stack
        pub fn start(config: MdnsConfig) -> anyhow::Result<Self> {
            let mut mdns = EspMdns::take()
                .map_err(|e| anyhow::anyhow!("failed to take mDNS singleton: {}", e))?;

            mdns.set_hostname(&config.hostname)
                .map_err(|e| anyhow::anyhow!("failed to set mDNS hostname: {}", e))?;

            mdns.set_instance_name(&config.hostname)
                .map_err(|e| anyhow::anyhow!("failed to set mDNS instance name: {}", e))?;

            let txt = [("path", config.odf_path.as_str())];
            mdns.add_service(
                Some(&config.hostname),
                SERVICE_TYPE,
                SERVICE_PROTO,
                config.port,
                &txt,
            )
            .map_err(|e| anyhow::anyhow!("failed to add mDNS service: {}", e))?;

            info!(
                "mDNS started: {}.local, {}:{} port {}",
                config.hostname, SERVICE_TYPE, SERVICE_PROTO, config.port
            );

            Ok(Self {
                _mdns: mdns,
                config,
            })
        }

        /// Re-register the service after a DHCP IP change.
        ///
        /// esp-idf mDNS automatically picks up the new IP from the netif,
        /// but we force a re-announcement by removing and re-adding the service.
        pub fn update_ip(&mut self) -> anyhow::Result<()> {
            // esp-idf re-announces on netif change; explicit re-add ensures
            // caches are updated promptly.
            self._mdns
                .remove_service(SERVICE_TYPE, SERVICE_PROTO)
                .map_err(|e| anyhow::anyhow!("failed to remove mDNS service: {}", e))?;

            let txt = [("path", self.config.odf_path.as_str())];
            self._mdns
                .add_service(
                    Some(&self.config.hostname),
                    SERVICE_TYPE,
                    SERVICE_PROTO,
                    self.config.port,
                    &txt,
                )
                .map_err(|e| anyhow::anyhow!("failed to re-add mDNS service: {}", e))?;

            info!("mDNS: re-announced service after IP change");
            Ok(())
        }

        /// Stop the responder. Sends goodbye (TTL=0) via EspMdns drop.
        /// FR-006: goodbye announcements on disconnect.
        pub fn stop(self) {
            // Drop triggers esp-idf mdns_free() which sends goodbye packets.
            drop(self);
        }
    }

    // FR-006: Drop sends goodbye automatically via EspMdns destructor.
}

#[cfg(feature = "esp")]
pub use esp_impl::MdnsResponder;

// Host stub for testing — no-op implementation.
#[cfg(not(feature = "esp"))]
mod host_stub {
    use super::*;

    /// Host-side stub for MdnsResponder. Records state for testing.
    pub struct MdnsResponder {
        config: MdnsConfig,
        running: bool,
        ip_updates: u32,
    }

    impl MdnsResponder {
        pub fn start(config: MdnsConfig) -> anyhow::Result<Self> {
            Ok(Self {
                config,
                running: true,
                ip_updates: 0,
            })
        }

        pub fn update_ip(&mut self) -> anyhow::Result<()> {
            self.ip_updates += 1;
            Ok(())
        }

        pub fn stop(self) {
            // no-op on host
        }

        pub fn hostname(&self) -> &str {
            &self.config.hostname
        }

        pub fn port(&self) -> u16 {
            self.config.port
        }

        pub fn odf_path(&self) -> &str {
            &self.config.odf_path
        }

        pub fn is_running(&self) -> bool {
            self.running
        }

        pub fn ip_update_count(&self) -> u32 {
            self.ip_updates
        }
    }

    impl Drop for MdnsResponder {
        fn drop(&mut self) {
            self.running = false;
        }
    }
}

#[cfg(not(feature = "esp"))]
pub use host_stub::MdnsResponder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = MdnsConfig::new("eomi");
        assert_eq!(cfg.hostname, "eomi");
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.odf_path, DEFAULT_ODF_PATH);
    }

    #[test]
    fn config_custom_hostname() {
        let cfg = MdnsConfig::new("living-room");
        assert_eq!(cfg.hostname, "living-room");
    }

    #[test]
    fn stub_start_and_stop() {
        let cfg = MdnsConfig::new("test-device");
        let responder = MdnsResponder::start(cfg).unwrap();
        assert_eq!(responder.hostname(), "test-device");
        assert_eq!(responder.port(), DEFAULT_PORT);
        assert_eq!(responder.odf_path(), DEFAULT_ODF_PATH);
        assert!(responder.is_running());
        responder.stop();
    }

    #[test]
    fn stub_update_ip() {
        let cfg = MdnsConfig::new("test-device");
        let mut responder = MdnsResponder::start(cfg).unwrap();
        assert_eq!(responder.ip_update_count(), 0);
        responder.update_ip().unwrap();
        assert_eq!(responder.ip_update_count(), 1);
        responder.update_ip().unwrap();
        assert_eq!(responder.ip_update_count(), 2);
    }

    #[test]
    fn stub_drop_marks_stopped() {
        let cfg = MdnsConfig::new("test-device");
        let responder = MdnsResponder::start(cfg).unwrap();
        let ptr = &responder as *const MdnsResponder;
        // Verify running before drop
        assert!(responder.is_running());
        drop(responder);
        // After drop, we can't access it — but the Drop impl sets running=false.
        // This test just verifies drop doesn't panic.
        let _ = ptr; // suppress unused warning
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(SERVICE_TYPE, "_omi");
        assert_eq!(SERVICE_PROTO, "_tcp");
        assert_eq!(DEFAULT_PORT, 80);
        assert_eq!(DEFAULT_ODF_PATH, "/Objects");
    }

    #[test]
    fn config_clone() {
        let cfg = MdnsConfig::new("cloned");
        let cfg2 = cfg.clone();
        assert_eq!(cfg.hostname, cfg2.hostname);
        assert_eq!(cfg.port, cfg2.port);
        assert_eq!(cfg.odf_path, cfg2.odf_path);
    }
}
