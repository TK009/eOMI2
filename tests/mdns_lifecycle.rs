// Host-side unit tests for mDNS responder lifecycle and periodic browsing.
//
// Tests the mDNS responder stub in combination with the WiFi state machine
// to verify correct start/stop behaviour across WiFi state transitions.
// Also tests MdnsBrowser integration with the discovery tree.

use reconfigurable_device::mdns::{
    MdnsConfig, MdnsResponder, DEFAULT_ODF_PATH, DEFAULT_PORT, SERVICE_PROTO, SERVICE_TYPE,
};
use reconfigurable_device::mdns_discovery::{
    BrowseConfig, MdnsBrowser, Peer, clear_peers, inject_peers,
};
use reconfigurable_device::wifi_sm::{WifiEvent, WifiSm, WifiSmConfig, WifiState};

fn test_sm_config() -> WifiSmConfig {
    WifiSmConfig {
        max_rotations: 3,
        initial_backoff_ms: 100,
        max_backoff_ms: 5000,
    }
}

// ---------------------------------------------------------------------------
// mDNS start/stop lifecycle correctness
// ---------------------------------------------------------------------------

#[test]
fn responder_start_sets_running() {
    let cfg = MdnsConfig::new("test-host");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert!(resp.is_running());
}

#[test]
fn responder_stop_is_clean() {
    let cfg = MdnsConfig::new("test-host");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert!(resp.is_running());
    resp.stop(); // should not panic
}

#[test]
fn responder_drop_marks_stopped() {
    let cfg = MdnsConfig::new("test-host");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert!(resp.is_running());
    drop(resp); // Drop impl sets running = false
}

#[test]
fn responder_start_stop_start_lifecycle() {
    // Simulates reconnect: start → stop → start again
    let cfg = MdnsConfig::new("device-a");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert!(resp.is_running());
    resp.stop();

    let cfg2 = MdnsConfig::new("device-a");
    let resp2 = MdnsResponder::start(cfg2).unwrap();
    assert!(resp2.is_running());
    resp2.stop();
}

// ---------------------------------------------------------------------------
// State machine transitions: Connected → start, Portal → stop, ConnectionLost → stop
// ---------------------------------------------------------------------------

/// Helper that mirrors main.rs mDNS lifecycle logic.
/// Returns the mDNS responder state after processing a wifi state change.
fn mdns_for_state(
    state: &WifiState,
    current: Option<MdnsResponder>,
    hostname: &str,
) -> Option<MdnsResponder> {
    match state {
        WifiState::Connected => {
            if current.is_none() {
                Some(MdnsResponder::start(MdnsConfig::new(hostname)).unwrap())
            } else {
                current
            }
        }
        _ => {
            // FR-007: mDNS MUST NOT be active outside Connected state
            if let Some(resp) = current {
                resp.stop();
            }
            None
        }
    }
}

#[test]
fn mdns_started_on_connected() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);

    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_some());
    assert!(resp.as_ref().unwrap().is_running());
}

#[test]
fn mdns_stopped_on_portal() {
    let mut sm = WifiSm::new(1, test_sm_config());
    // Connect first
    sm.handle_event(WifiEvent::ConnectSuccess);
    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_some());

    // Exhaust rotations to reach Portal
    sm.handle_event(WifiEvent::ConnectionLost);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Portal);

    let resp = mdns_for_state(sm.state(), resp, "my-device");
    assert!(resp.is_none());
}

#[test]
fn mdns_stopped_on_connection_lost() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_some());

    // Connection lost → transitions to Connecting
    sm.handle_event(WifiEvent::ConnectionLost);
    assert!(matches!(*sm.state(), WifiState::Connecting { .. }));

    let resp = mdns_for_state(sm.state(), resp, "my-device");
    assert!(resp.is_none());
}

#[test]
fn mdns_restarted_after_reconnect() {
    let mut sm = WifiSm::new(1, test_sm_config());

    // First connection
    sm.handle_event(WifiEvent::ConnectSuccess);
    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_some());

    // Disconnect
    sm.handle_event(WifiEvent::ConnectionLost);
    let resp = mdns_for_state(sm.state(), resp, "my-device");
    assert!(resp.is_none());

    // Reconnect
    sm.handle_event(WifiEvent::ConnectSuccess);
    let resp = mdns_for_state(sm.state(), resp, "my-device");
    assert!(resp.is_some());
    assert!(resp.as_ref().unwrap().is_running());
}

// ---------------------------------------------------------------------------
// FR-007: mDNS NOT started in AP mode / Unconfigured state
// ---------------------------------------------------------------------------

#[test]
fn mdns_not_started_when_unconfigured() {
    let sm = WifiSm::new(0, test_sm_config());
    assert_eq!(*sm.state(), WifiState::Unconfigured);

    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_none());
}

#[test]
fn mdns_not_started_during_connecting() {
    let sm = WifiSm::new(1, test_sm_config());
    assert!(matches!(*sm.state(), WifiState::Connecting { .. }));

    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_none());
}

#[test]
fn mdns_not_started_during_backoff() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectFailed);
    assert!(matches!(*sm.state(), WifiState::Backoff));

    let resp = mdns_for_state(sm.state(), None, "my-device");
    assert!(resp.is_none());
}

#[test]
fn mdns_stopped_in_every_non_connected_state() {
    let hostname = "test-dev";

    // Start with a running responder
    let make_running = || Some(MdnsResponder::start(MdnsConfig::new(hostname)).unwrap());

    // Unconfigured
    let resp = mdns_for_state(&WifiState::Unconfigured, make_running(), hostname);
    assert!(resp.is_none());

    // Connecting
    let resp = mdns_for_state(
        &WifiState::Connecting { ssid_index: 0 },
        make_running(),
        hostname,
    );
    assert!(resp.is_none());

    // Backoff
    let resp = mdns_for_state(&WifiState::Backoff, make_running(), hostname);
    assert!(resp.is_none());

    // Portal
    let resp = mdns_for_state(&WifiState::Portal, make_running(), hostname);
    assert!(resp.is_none());
}

// ---------------------------------------------------------------------------
// Hostname from wifi_cfg used correctly
// ---------------------------------------------------------------------------

#[test]
fn responder_uses_configured_hostname() {
    let cfg = MdnsConfig::new("living-room");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert_eq!(resp.hostname(), "living-room");
}

#[test]
fn responder_uses_custom_hostname_from_config() {
    let cfg = MdnsConfig::new("kitchen-sensor");
    let resp = MdnsResponder::start(cfg).unwrap();
    assert_eq!(resp.hostname(), "kitchen-sensor");
    assert_eq!(resp.port(), DEFAULT_PORT);
    assert_eq!(resp.odf_path(), DEFAULT_ODF_PATH);
}

#[test]
fn mdns_hostname_matches_wifi_cfg_hostname() {
    // Simulates the main loop pattern: hostname comes from WifiConfig
    let hostname = "my-custom-host";
    let mdns_cfg = MdnsConfig::new(hostname);
    let resp = MdnsResponder::start(mdns_cfg).unwrap();
    assert_eq!(resp.hostname(), hostname);
}

#[test]
fn mdns_config_preserves_custom_port_and_path() {
    let mut cfg = MdnsConfig::new("dev");
    cfg.port = 8080;
    cfg.odf_path = "/Objects/Sensors".to_string();
    let resp = MdnsResponder::start(cfg).unwrap();
    assert_eq!(resp.port(), 8080);
    assert_eq!(resp.odf_path(), "/Objects/Sensors");
}

// ---------------------------------------------------------------------------
// update_ip() called on IP change
// ---------------------------------------------------------------------------

#[test]
fn update_ip_increments_counter() {
    let cfg = MdnsConfig::new("test-dev");
    let mut resp = MdnsResponder::start(cfg).unwrap();
    assert_eq!(resp.ip_update_count(), 0);

    resp.update_ip().unwrap();
    assert_eq!(resp.ip_update_count(), 1);
}

#[test]
fn update_ip_multiple_times() {
    let cfg = MdnsConfig::new("test-dev");
    let mut resp = MdnsResponder::start(cfg).unwrap();

    for i in 1..=5 {
        resp.update_ip().unwrap();
        assert_eq!(resp.ip_update_count(), i);
    }
}

#[test]
fn update_ip_simulated_dhcp_renewal() {
    // Simulate the main loop pattern: IP changed → call update_ip
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);

    let cfg = MdnsConfig::new("test-dev");
    let mut resp = MdnsResponder::start(cfg).unwrap();
    let mut last_ip: Option<String> = None;

    // Initial IP assignment
    let current_ip = "192.168.1.100".to_string();
    if last_ip.as_deref() != Some(&current_ip) {
        last_ip = Some(current_ip);
        resp.update_ip().unwrap();
    }
    assert_eq!(resp.ip_update_count(), 1);

    // Same IP — no update
    let current_ip = "192.168.1.100".to_string();
    if last_ip.as_deref() != Some(&current_ip) {
        resp.update_ip().unwrap();
    }
    assert_eq!(resp.ip_update_count(), 1); // unchanged

    // DHCP renewal — new IP
    let current_ip = "192.168.1.200".to_string();
    if last_ip.as_deref() != Some(&current_ip) {
        let _ = last_ip.insert(current_ip);
        resp.update_ip().unwrap();
    }
    assert_eq!(resp.ip_update_count(), 2);
}

// ---------------------------------------------------------------------------
// Full lifecycle: connect → IP → disconnect → reconnect → new IP
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_connect_ip_disconnect_reconnect() {
    let hostname = "lifecycle-test";
    let mut sm = WifiSm::new(2, test_sm_config());
    let mut mdns: Option<MdnsResponder> = None;
    let mut last_ip: Option<String> = None;

    // 1. Connect
    sm.handle_event(WifiEvent::ConnectSuccess);
    mdns = mdns_for_state(sm.state(), mdns, hostname);
    assert!(mdns.is_some());

    // 2. Get initial IP
    let ip = "10.0.0.5";
    if last_ip.as_deref() != Some(ip) {
        let _ = last_ip.insert(ip.to_string());
        mdns.as_mut().unwrap().update_ip().unwrap();
    }
    assert_eq!(mdns.as_ref().unwrap().ip_update_count(), 1);

    // 3. Connection lost
    sm.handle_event(WifiEvent::ConnectionLost);
    mdns = mdns_for_state(sm.state(), mdns, hostname);
    assert!(mdns.is_none());
    last_ip = None;

    // 4. Reconnect
    sm.handle_event(WifiEvent::ConnectSuccess);
    mdns = mdns_for_state(sm.state(), mdns, hostname);
    assert!(mdns.is_some());
    // Fresh responder — counter reset
    assert_eq!(mdns.as_ref().unwrap().ip_update_count(), 0);

    // 5. New IP on reconnect
    let ip = "10.0.0.42";
    if last_ip.as_deref() != Some(ip) {
        let _ = last_ip.insert(ip.to_string());
        mdns.as_mut().unwrap().update_ip().unwrap();
    }
    assert_eq!(mdns.as_ref().unwrap().ip_update_count(), 1);
    assert_eq!(mdns.as_ref().unwrap().hostname(), hostname);
}

// ---------------------------------------------------------------------------
// Constants sanity
// ---------------------------------------------------------------------------

#[test]
fn service_constants_match_omi_spec() {
    assert_eq!(SERVICE_TYPE, "_omi");
    assert_eq!(SERVICE_PROTO, "_tcp");
    assert_eq!(DEFAULT_PORT, 80);
    assert_eq!(DEFAULT_ODF_PATH, "/Objects");
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn mdns_not_double_started_when_already_running() {
    // mdns_for_state should not create a second responder if one exists
    let hostname = "no-double";
    let resp = MdnsResponder::start(MdnsConfig::new(hostname)).unwrap();
    let resp = mdns_for_state(&WifiState::Connected, Some(resp), hostname);
    assert!(resp.is_some());
    // Still the same responder (ip_update_count == 0, not reset by re-creation)
    assert_eq!(resp.as_ref().unwrap().ip_update_count(), 0);
}

#[test]
fn mdns_stop_when_none_is_noop() {
    // Stopping when no responder exists should not panic
    let resp = mdns_for_state(&WifiState::Portal, None, "test");
    assert!(resp.is_none());
}

// ---------------------------------------------------------------------------
// FR-008: Periodic DNS-SD browsing via MdnsBrowser
// ---------------------------------------------------------------------------

#[test]
fn browser_only_active_when_connected() {
    // MdnsBrowser should only be ticked when in Connected state.
    // This mirrors the main.rs pattern: create browser on connect, drop on disconnect.
    let mut sm = WifiSm::new(1, test_sm_config());

    // Not connected yet — no browser
    let browser: Option<MdnsBrowser> = None;
    assert!(browser.is_none());

    // Connect
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);
    let mut browser = Some(MdnsBrowser::new(BrowseConfig::default()));

    // Browser fires on first tick
    clear_peers();
    let result = browser.as_mut().unwrap().tick(0);
    assert!(result.is_some());

    // Disconnect — drop browser
    sm.handle_event(WifiEvent::ConnectionLost);
    drop(browser);

    // Reconnect — new browser
    sm.handle_event(WifiEvent::ConnectSuccess);
    let browser = MdnsBrowser::new(BrowseConfig::default());
    assert_eq!(browser.cycle_count(), 0);
}

#[test]
fn browser_discovers_peers_on_tick() {
    clear_peers();
    inject_peers(vec![
        Peer { hostname: "kitchen".into(), ip: "192.168.1.10".into(), port: 80 },
        Peer { hostname: "garage".into(), ip: "192.168.1.11".into(), port: 8080 },
    ]);

    let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(10_000));
    let result = browser.tick(0).unwrap(); // initial trigger
    assert_eq!(result.peers.len(), 2);
    assert_eq!(result.peers[0].hostname, "kitchen");
    assert_eq!(result.peers[1].hostname, "garage");

    clear_peers();
}

#[test]
fn browser_interval_respects_config() {
    clear_peers();
    let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(60_000));
    assert_eq!(browser.interval_ms(), 60_000);

    browser.tick(0); // initial
    // 30s is not enough for 60s interval
    assert!(browser.tick(30_000).is_none());
    // 30s more = 60s total
    assert!(browser.tick(30_000).is_some());
}

#[test]
fn browser_lifecycle_with_wifi_transitions() {
    // Simulates: connect → browse → disconnect → reconnect → browse
    let mut sm = WifiSm::new(1, test_sm_config());

    clear_peers();
    inject_peers(vec![
        Peer { hostname: "bedroom".into(), ip: "10.0.0.5".into(), port: 80 },
    ]);

    // Connect and start browsing
    sm.handle_event(WifiEvent::ConnectSuccess);
    let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(5_000));
    let r1 = browser.tick(0).unwrap();
    assert_eq!(r1.peers.len(), 1);
    assert_eq!(r1.cycle_count, 1);

    // Disconnect — stop browsing
    sm.handle_event(WifiEvent::ConnectionLost);
    drop(browser);

    // Reconnect — new browser, cycle count resets
    sm.handle_event(WifiEvent::ConnectSuccess);
    let mut browser = MdnsBrowser::new(BrowseConfig::with_interval_ms(5_000));
    let r2 = browser.tick(0).unwrap();
    assert_eq!(r2.cycle_count, 1); // fresh browser

    clear_peers();
}

#[test]
fn browser_not_started_in_portal_mode() {
    let mut sm = WifiSm::new(1, test_sm_config());
    // Exhaust rotations to reach Portal
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Portal);

    // Browser should NOT be created in portal mode (FR-007)
    let browser: Option<MdnsBrowser> = match sm.state() {
        WifiState::Connected => Some(MdnsBrowser::new(BrowseConfig::default())),
        _ => None,
    };
    assert!(browser.is_none());
}
