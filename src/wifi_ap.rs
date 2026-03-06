// Soft-AP mode for captive portal provisioning (FR-017).
//
// Pure helpers (ap_ssid, AP_IP) are platform-independent and testable on host.
// ESP-specific functions (start_ap, stop_ap, try_connect_sta) are gated
// behind #[cfg(feature = "esp")].

/// Default AP IP — ESP-IDF assigns 192.168.4.1 to the AP netif by default.
pub const AP_IP: &str = "192.168.4.1";

/// Maximum SSID length for the AP (IEEE 802.11 limit is 32 bytes).
const MAX_AP_SSID_LEN: usize = 32;

/// Build the AP SSID from the hostname: "setup-{hostname}", truncated to 32 bytes.
pub fn ap_ssid(hostname: &str) -> String {
    let prefix = "setup-";
    let max_host = MAX_AP_SSID_LEN - prefix.len();
    let host = if hostname.len() > max_host {
        &hostname[..max_host]
    } else {
        hostname
    };
    let mut ssid = String::with_capacity(prefix.len() + host.len());
    ssid.push_str(prefix);
    ssid.push_str(host);
    ssid
}

#[cfg(feature = "esp")]
mod esp_impl {
    use super::*;
    use esp_idf_svc::wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration,
        EspWifi,
    };
    use log::info;

    fn ap_config(hostname: &str) -> anyhow::Result<AccessPointConfiguration> {
        let ssid = ap_ssid(hostname);
        Ok(AccessPointConfiguration {
            ssid: ssid
                .as_str()
                .try_into()
                .map_err(|_| anyhow::anyhow!("AP SSID too long"))?,
            auth_method: AuthMethod::None,
            channel: 1,
            max_connections: 4,
            ..Default::default()
        })
    }

    /// Start the soft-AP in AP+STA (mixed) mode.
    ///
    /// The STA side is left unconfigured (empty SSID) so the AP comes up
    /// without attempting a station connection. The caller can later set
    /// STA credentials and connect via `try_connect_sta`.
    pub fn start_ap(
        wifi: &mut BlockingWifi<EspWifi<'static>>,
        hostname: &str,
    ) -> anyhow::Result<()> {
        info!("Starting soft-AP: SSID={}", ap_ssid(hostname));

        wifi.set_configuration(&Configuration::Mixed(
            ClientConfiguration::default(),
            ap_config(hostname)?,
        ))?;

        wifi.start()?;
        wifi.wait_netif_up()?;

        info!("Soft-AP started on {}", AP_IP);
        Ok(())
    }

    /// Stop the soft-AP by switching back to STA-only mode.
    ///
    /// After calling this the AP interface is no longer active and connected
    /// clients are dropped.
    pub fn stop_ap(wifi: &mut BlockingWifi<EspWifi<'static>>) -> anyhow::Result<()> {
        info!("Stopping soft-AP");
        wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
        wifi.start()?;
        info!("Soft-AP stopped");
        Ok(())
    }

    /// Try connecting the STA side while AP is running (mixed mode).
    ///
    /// Sets the mixed configuration with the given STA credentials and the
    /// existing AP config, then connects the STA interface.
    pub fn try_connect_sta(
        wifi: &mut BlockingWifi<EspWifi<'static>>,
        ssid: &str,
        password: &str,
        ap_hostname: &str,
    ) -> anyhow::Result<()> {
        let sta_config = ClientConfiguration {
            ssid: ssid
                .try_into()
                .map_err(|_| anyhow::anyhow!("SSID too long"))?,
            password: password
                .try_into()
                .map_err(|_| anyhow::anyhow!("Password too long"))?,
            ..Default::default()
        };

        wifi.set_configuration(&Configuration::Mixed(sta_config, ap_config(ap_hostname)?))?;
        wifi.start()?;
        wifi.connect()?;
        wifi.wait_netif_up()?;

        Ok(())
    }
}

#[cfg(feature = "esp")]
pub use esp_impl::{start_ap, stop_ap, try_connect_sta};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_ssid_basic() {
        assert_eq!(ap_ssid("eOMI"), "setup-eOMI");
    }

    #[test]
    fn ap_ssid_empty_hostname() {
        assert_eq!(ap_ssid(""), "setup-");
    }

    #[test]
    fn ap_ssid_truncates_long_hostname() {
        let long = "a".repeat(40);
        let result = ap_ssid(&long);
        assert_eq!(result.len(), MAX_AP_SSID_LEN);
        assert!(result.starts_with("setup-"));
    }

    #[test]
    fn ap_ssid_exact_max() {
        let host = "a".repeat(MAX_AP_SSID_LEN - "setup-".len());
        let result = ap_ssid(&host);
        assert_eq!(result.len(), MAX_AP_SSID_LEN);
    }
}
