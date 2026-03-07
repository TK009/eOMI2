// DNS-SD peer discovery for _omi._tcp services on the LAN.
//
// Queries the mDNS responder for peers advertising _omi._tcp and returns
// a list of discovered devices. Platform-gated: ESP uses esp_idf_svc::mdns
// query API; host stub returns an empty list (or injected test data).

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
}
