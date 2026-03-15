// DNS-SD peer discovery for _omi._tcp services on the LAN.
//
// Queries the mDNS responder for peers advertising _omi._tcp and returns
// a list of discovered devices. Platform-gated: ESP uses esp_idf_svc::mdns
// query API; host stub returns an empty list (or injected test data).
//
// MdnsBrowser provides periodic service browsing (FR-008): it tracks elapsed
// time and triggers discovery at a configurable interval (default 30s).
// Only active when explicitly ticked — the caller controls when browsing
// is appropriate (e.g., only in station mode, not AP).

/// A discovered eOMI peer on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub hostname: String,
    pub ip: String,
    pub port: u16,
}

#[cfg(feature = "esp")]
mod esp_impl {
    use super::*;
    use crate::mdns::{MdnsResponder, SERVICE_TYPE, SERVICE_PROTO};
    use core::time::Duration;
    use esp_idf_svc::mdns::QueryResult;
    use log::debug;

    /// Maximum number of results from a single mDNS query.
    const MAX_RESULTS: usize = 8;

    /// Default query timeout.
    const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

    fn empty_result() -> QueryResult {
        QueryResult {
            instance_name: None,
            hostname: None,
            port: 0,
            txt: Vec::new(),
            addr: Vec::new(),
            interface: esp_idf_svc::mdns::Interface::STA,
            #[cfg(esp_idf_lwip_ipv4)]
            ip_protocol: esp_idf_svc::mdns::Protocol::V4,
            #[cfg(all(esp_idf_lwip_ipv6, not(esp_idf_lwip_ipv4)))]
            ip_protocol: esp_idf_svc::mdns::Protocol::V6,
        }
    }

    /// Browse for _omi._tcp peers via mDNS PTR query.
    ///
    /// Returns discovered peers with their hostname, IP address, and port.
    /// Filters out results that lack an IP address (incomplete responses).
    pub fn discover_peers(responder: &MdnsResponder) -> Vec<Peer> {
        let mdns = responder.inner();
        let mut buf: Vec<QueryResult> = (0..MAX_RESULTS).map(|_| empty_result()).collect();

        let count = match mdns.query_ptr(
            SERVICE_TYPE,
            SERVICE_PROTO,
            QUERY_TIMEOUT,
            MAX_RESULTS,
            &mut buf,
        ) {
            Ok(n) => n,
            Err(e) => {
                debug!("mDNS discovery query failed: {}", e);
                return Vec::new();
            }
        };

        let mut peers = Vec::with_capacity(count);
        for result in &buf[..count] {
            let ip = match result.addr.first() {
                Some(addr) => format!("{}", addr),
                None => continue,
            };

            let hostname = result
                .hostname
                .clone()
                .unwrap_or_default();

            peers.push(Peer {
                hostname,
                ip,
                port: result.port,
            });
        }

        debug!("mDNS discovery: found {} peers", peers.len());
        peers
    }
}

#[cfg(feature = "esp")]
pub use esp_impl::discover_peers;

#[cfg(not(feature = "esp"))]
mod host_stub {
    use super::*;
    use std::sync::Mutex;

    static INJECTED_PEERS: Mutex<Vec<Peer>> = Mutex::new(Vec::new());

    /// Host stub: returns injected test peers (empty by default).
    pub fn discover_peers() -> Vec<Peer> {
        INJECTED_PEERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Inject peers for testing. Not available on ESP.
    pub fn inject_peers(peers: Vec<Peer>) {
        *INJECTED_PEERS
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = peers;
    }

    /// Clear injected peers.
    pub fn clear_peers() {
        inject_peers(Vec::new());
    }
}

#[cfg(not(feature = "esp"))]
pub use host_stub::{clear_peers, discover_peers, inject_peers};

/// Default periodic browse interval in milliseconds (30s per spec).
pub const DEFAULT_BROWSE_INTERVAL_MS: u64 = 30_000;

/// Minimum allowed browse interval (5s) to prevent excessive network traffic.
pub const MIN_BROWSE_INTERVAL_MS: u64 = 5_000;

/// Maximum allowed browse interval (5min).
pub const MAX_BROWSE_INTERVAL_MS: u64 = 300_000;

/// Configuration for periodic mDNS service browsing.
#[derive(Debug, Clone)]
pub struct BrowseConfig {
    /// Interval between browse queries in milliseconds.
    pub interval_ms: u64,
}

impl Default for BrowseConfig {
    fn default() -> Self {
        Self {
            interval_ms: DEFAULT_BROWSE_INTERVAL_MS,
        }
    }
}

impl BrowseConfig {
    /// Create a new config with the given interval, clamped to valid range.
    pub fn with_interval_ms(interval_ms: u64) -> Self {
        Self {
            interval_ms: interval_ms.clamp(MIN_BROWSE_INTERVAL_MS, MAX_BROWSE_INTERVAL_MS),
        }
    }
}

/// Result of a single browse cycle.
#[derive(Debug, Clone)]
pub struct BrowseResult {
    /// Peers discovered in this cycle.
    pub peers: Vec<Peer>,
    /// Total number of browse cycles completed (including this one).
    pub cycle_count: u64,
}

/// Periodic DNS-SD service browser for _omi._tcp peer discovery (FR-008).
///
/// Tracks elapsed time and triggers `discover_peers()` at the configured
/// interval. The caller is responsible for calling `tick()` regularly
/// (e.g., from the main loop) and only when browsing is appropriate
/// (station mode, not AP mode — per FR-007).
///
/// # Usage
/// ```ignore
/// let mut browser = MdnsBrowser::new(BrowseConfig::default());
///
/// // In main loop (every 100ms poll):
/// if let Some(result) = browser.tick(100) {
///     // result.peers contains discovered peers
///     update_discovery_tree(&mut tree, &result.peers, Some(now));
/// }
/// ```
pub struct MdnsBrowser {
    config: BrowseConfig,
    elapsed_ms: u64,
    cycle_count: u64,
}

impl MdnsBrowser {
    /// Create a new periodic browser. First browse triggers immediately
    /// on the first `tick()` call (elapsed starts at interval).
    pub fn new(config: BrowseConfig) -> Self {
        Self {
            elapsed_ms: config.interval_ms, // trigger on first tick
            config,
            cycle_count: 0,
        }
    }

    /// Advance the timer by `delta_ms` milliseconds.
    ///
    /// Returns `Some(BrowseResult)` when a browse cycle fires, `None` otherwise.
    /// On ESP, the caller must pass the `MdnsResponder` via `tick_with_responder`.
    /// On host, this calls the stub `discover_peers()` directly.
    #[cfg(not(feature = "esp"))]
    pub fn tick(&mut self, delta_ms: u64) -> Option<BrowseResult> {
        self.elapsed_ms += delta_ms;
        if self.elapsed_ms >= self.config.interval_ms {
            self.elapsed_ms = 0;
            self.cycle_count += 1;
            Some(BrowseResult {
                peers: discover_peers(),
                cycle_count: self.cycle_count,
            })
        } else {
            None
        }
    }

    /// ESP variant: advance timer and browse using the given responder.
    #[cfg(feature = "esp")]
    pub fn tick(&mut self, delta_ms: u64, responder: &crate::mdns::MdnsResponder) -> Option<BrowseResult> {
        self.elapsed_ms += delta_ms;
        if self.elapsed_ms >= self.config.interval_ms {
            self.elapsed_ms = 0;
            self.cycle_count += 1;
            Some(BrowseResult {
                peers: discover_peers(responder),
                cycle_count: self.cycle_count,
            })
        } else {
            None
        }
    }

    /// Reset the timer. Next browse fires after a full interval.
    pub fn reset(&mut self) {
        self.elapsed_ms = 0;
    }

    /// Number of completed browse cycles.
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }

    /// Current browse interval in milliseconds.
    pub fn interval_ms(&self) -> u64 {
        self.config.interval_ms
    }

    /// Milliseconds remaining until the next browse.
    pub fn remaining_ms(&self) -> u64 {
        self.config.interval_ms.saturating_sub(self.elapsed_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_struct_clone_and_eq() {
        let p1 = Peer {
            hostname: "kitchen".into(),
            ip: "192.168.1.10".into(),
            port: 80,
        };
        let p2 = p1.clone();
        assert_eq!(p1, p2);
    }

    #[test]
    fn peer_debug_format() {
        let p = Peer {
            hostname: "test".into(),
            ip: "10.0.0.1".into(),
            port: 8080,
        };
        let dbg = format!("{:?}", p);
        assert!(dbg.contains("test"));
        assert!(dbg.contains("10.0.0.1"));
        assert!(dbg.contains("8080"));
    }

    #[test]
    fn stub_discover_empty_by_default() {
        clear_peers();
        let peers = discover_peers();
        assert!(peers.is_empty());
    }

    #[test]
    fn stub_inject_and_discover() {
        let injected = vec![
            Peer {
                hostname: "living-room".into(),
                ip: "192.168.1.20".into(),
                port: 80,
            },
            Peer {
                hostname: "garage".into(),
                ip: "192.168.1.21".into(),
                port: 8080,
            },
        ];
        inject_peers(injected.clone());
        let found = discover_peers();
        assert_eq!(found, injected);

        clear_peers();
        assert!(discover_peers().is_empty());
    }

    #[test]
    fn stub_inject_overwrites_previous() {
        inject_peers(vec![Peer {
            hostname: "a".into(),
            ip: "1.2.3.4".into(),
            port: 80,
        }]);
        assert_eq!(discover_peers().len(), 1);

        inject_peers(vec![
            Peer {
                hostname: "b".into(),
                ip: "5.6.7.8".into(),
                port: 80,
            },
            Peer {
                hostname: "c".into(),
                ip: "9.10.11.12".into(),
                port: 80,
            },
        ]);
        assert_eq!(discover_peers().len(), 2);
        assert_eq!(discover_peers()[0].hostname, "b");

        clear_peers();
    }

    // --- BrowseConfig tests ---

    #[test]
    fn browse_config_default_interval() {
        let cfg = BrowseConfig::default();
        assert_eq!(cfg.interval_ms, DEFAULT_BROWSE_INTERVAL_MS);
    }

    #[test]
    fn browse_config_custom_interval() {
        let cfg = BrowseConfig::with_interval_ms(60_000);
        assert_eq!(cfg.interval_ms, 60_000);
    }

    #[test]
    fn browse_config_clamps_below_minimum() {
        let cfg = BrowseConfig::with_interval_ms(100);
        assert_eq!(cfg.interval_ms, MIN_BROWSE_INTERVAL_MS);
    }

    #[test]
    fn browse_config_clamps_above_maximum() {
        let cfg = BrowseConfig::with_interval_ms(999_999);
        assert_eq!(cfg.interval_ms, MAX_BROWSE_INTERVAL_MS);
    }

    #[test]
    fn browse_config_at_boundary_values() {
        assert_eq!(BrowseConfig::with_interval_ms(MIN_BROWSE_INTERVAL_MS).interval_ms, MIN_BROWSE_INTERVAL_MS);
        assert_eq!(BrowseConfig::with_interval_ms(MAX_BROWSE_INTERVAL_MS).interval_ms, MAX_BROWSE_INTERVAL_MS);
    }

    // --- MdnsBrowser tests ---

    #[test]
    fn browser_fires_on_first_tick() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::default());
        // First tick with any delta should trigger (elapsed starts at interval)
        let result = browser.tick(0);
        assert!(result.is_some());
        assert_eq!(result.unwrap().cycle_count, 1);
    }

    #[test]
    fn browser_does_not_fire_before_interval() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
        // Consume the initial trigger
        browser.tick(0);

        // Not enough time has passed
        assert!(browser.tick(5_000).is_none());
        assert!(browser.tick(4_999).is_none());
    }

    #[test]
    fn browser_fires_at_interval() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
        browser.tick(0); // consume initial

        // Accumulate to exactly the interval
        assert!(browser.tick(5_000).is_none());
        let result = browser.tick(5_000);
        assert!(result.is_some());
        assert_eq!(result.unwrap().cycle_count, 2);
    }

    #[test]
    fn browser_fires_past_interval() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
        browser.tick(0); // consume initial

        // Overshoot the interval
        let result = browser.tick(15_000);
        assert!(result.is_some());
    }

    #[test]
    fn browser_returns_injected_peers() {
        let peers = vec![
            Peer { hostname: "kitchen".into(), ip: "192.168.1.10".into(), port: 80 },
            Peer { hostname: "garage".into(), ip: "192.168.1.11".into(), port: 80 },
        ];
        inject_peers(peers.clone());

        let mut browser = MdnsBrowser::new(BrowseConfig::default());
        let result = browser.tick(0).unwrap();
        assert_eq!(result.peers, peers);

        clear_peers();
    }

    #[test]
    fn browser_cycle_count_increments() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(5_000));
        assert_eq!(browser.cycle_count(), 0);

        browser.tick(0); // first
        assert_eq!(browser.cycle_count(), 1);

        browser.tick(5_000); // second
        assert_eq!(browser.cycle_count(), 2);

        browser.tick(5_000); // third
        assert_eq!(browser.cycle_count(), 3);
    }

    #[test]
    fn browser_reset_delays_next_browse() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
        browser.tick(0); // consume initial

        browser.tick(8_000); // 8s elapsed
        browser.reset(); // reset to 0

        // Need full interval again
        assert!(browser.tick(8_000).is_none());
        assert!(browser.tick(2_000).is_some());
    }

    #[test]
    fn browser_remaining_ms() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
        browser.tick(0); // consume initial, elapsed resets to 0

        assert_eq!(browser.remaining_ms(), 10_000);
        browser.tick(3_000); // no fire, 3s elapsed
        assert_eq!(browser.remaining_ms(), 7_000);
    }

    #[test]
    fn browser_interval_ms_matches_config() {
        let browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(45_000));
        assert_eq!(browser.interval_ms(), 45_000);
    }

    #[test]
    fn browser_multiple_cycles_with_varying_deltas() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(5_000));
        browser.tick(0); // initial

        // Simulate irregular tick intervals
        assert!(browser.tick(1_000).is_none());
        assert!(browser.tick(2_000).is_none());
        assert!(browser.tick(1_000).is_none());
        assert!(browser.tick(1_000).is_some()); // 5s total
        assert_eq!(browser.cycle_count(), 2);
    }

    #[test]
    fn browser_reflects_peer_changes_between_cycles() {
        clear_peers();
        let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(5_000));

        // First cycle: no peers
        let r1 = browser.tick(0).unwrap();
        assert!(r1.peers.is_empty());

        // Inject peers before next cycle
        inject_peers(vec![Peer {
            hostname: "bedroom".into(),
            ip: "10.0.0.5".into(),
            port: 80,
        }]);

        let r2 = browser.tick(5_000).unwrap();
        assert_eq!(r2.peers.len(), 1);
        assert_eq!(r2.peers[0].hostname, "bedroom");

        // Remove peers
        clear_peers();
        let r3 = browser.tick(5_000).unwrap();
        assert!(r3.peers.is_empty());
    }
}
