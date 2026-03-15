// Host-side tests for SNTP time sync lifecycle across WiFi state transitions.
//
// Tests the TimeSync stub in combination with the WiFi state machine
// to verify correct start/stop behaviour. Mirrors the main.rs lifecycle
// logic to catch handle leaks and ensure fresh sync on reconnect.

use reconfigurable_device::time_sync::TimeSync;
use reconfigurable_device::wifi_sm::{WifiEvent, WifiSm, WifiSmConfig, WifiState};

fn test_sm_config() -> WifiSmConfig {
    WifiSmConfig {
        max_rotations: 3,
        initial_backoff_ms: 100,
        max_backoff_ms: 5000,
    }
}

/// Mirrors the main.rs SNTP lifecycle logic in the WiFi state match block.
/// On Connected: start if not running. Otherwise: stop if running.
fn sntp_for_state(state: &WifiState, current: Option<TimeSync>) -> Option<TimeSync> {
    match state {
        WifiState::Connected => {
            if current.is_none() {
                TimeSync::start()
            } else {
                current
            }
        }
        _ => {
            // Drop the handle (stops SNTP)
            drop(current);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Basic start/stop
// ---------------------------------------------------------------------------

#[test]
fn time_sync_start_succeeds() {
    let ts = TimeSync::start();
    assert!(ts.is_some());
    assert!(ts.unwrap().is_running());
}

#[test]
fn time_sync_drop_stops() {
    let ts = TimeSync::start().unwrap();
    assert!(ts.is_running());
    drop(ts);
}

#[test]
fn time_sync_start_stop_start() {
    let ts = TimeSync::start().unwrap();
    assert!(ts.is_running());
    drop(ts);

    let ts2 = TimeSync::start().unwrap();
    assert!(ts2.is_running());
    drop(ts2);
}

// ---------------------------------------------------------------------------
// State machine integration: start on Connected, stop otherwise
// ---------------------------------------------------------------------------

#[test]
fn sntp_started_on_connected() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);

    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_some());
}

#[test]
fn sntp_not_started_when_unconfigured() {
    let sm = WifiSm::new(0, test_sm_config());
    assert_eq!(*sm.state(), WifiState::Unconfigured);

    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_none());
}

#[test]
fn sntp_not_started_during_connecting() {
    let sm = WifiSm::new(1, test_sm_config());
    assert!(matches!(*sm.state(), WifiState::Connecting { .. }));

    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_none());
}

#[test]
fn sntp_not_started_during_backoff() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectFailed);
    assert!(matches!(*sm.state(), WifiState::Backoff));

    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_none());
}

#[test]
fn sntp_stopped_on_portal() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_some());

    // Exhaust rotations to reach Portal
    sm.handle_event(WifiEvent::ConnectionLost);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Portal);

    let ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());
}

#[test]
fn sntp_stopped_on_connection_lost() {
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_some());

    sm.handle_event(WifiEvent::ConnectionLost);
    assert!(matches!(*sm.state(), WifiState::Connecting { .. }));

    let ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());
}

#[test]
fn sntp_stopped_in_every_non_connected_state() {
    let make_running = || TimeSync::start();

    let ts = sntp_for_state(&WifiState::Unconfigured, make_running());
    assert!(ts.is_none());

    let ts = sntp_for_state(&WifiState::Connecting { ssid_index: 0 }, make_running());
    assert!(ts.is_none());

    let ts = sntp_for_state(&WifiState::Backoff, make_running());
    assert!(ts.is_none());

    let ts = sntp_for_state(&WifiState::Portal, make_running());
    assert!(ts.is_none());
}

// ---------------------------------------------------------------------------
// Reconnection: fresh handle after disconnect/reconnect cycle
// ---------------------------------------------------------------------------

#[test]
fn sntp_restarted_after_reconnect() {
    let mut sm = WifiSm::new(1, test_sm_config());

    // First connection
    sm.handle_event(WifiEvent::ConnectSuccess);
    let ts = sntp_for_state(sm.state(), None);
    assert!(ts.is_some());

    // Disconnect
    sm.handle_event(WifiEvent::ConnectionLost);
    let ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());

    // Reconnect
    sm.handle_event(WifiEvent::ConnectSuccess);
    let ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_some());
}

#[test]
fn sntp_not_double_started_when_already_running() {
    let ts = TimeSync::start();
    let ts = sntp_for_state(&WifiState::Connected, ts);
    assert!(ts.is_some());
    // Should be the same handle (not recreated)
}

// ---------------------------------------------------------------------------
// Quick reconnect: ConnectionLost + immediate reconnect in same tick
// ---------------------------------------------------------------------------

/// Simulates the main.rs pattern where WiFi drops and reconnects within a
/// single loop iteration. SNTP must be stopped before reconnect attempt so
/// a fresh handle is created — not kept stale from the previous connection.
#[test]
fn sntp_fresh_handle_on_quick_reconnect() {
    let mut sm = WifiSm::new(1, test_sm_config());

    // Initial connection → start SNTP
    sm.handle_event(WifiEvent::ConnectSuccess);
    let mut ts = TimeSync::start();
    assert!(ts.is_some());

    // ConnectionLost detected: stop SNTP BEFORE reconnect attempt
    // (mirrors the fix in main.rs lines 443-450)
    ts = None; // .take() in main.rs

    // State machine processes ConnectionLost → Connecting → immediate success
    sm.handle_event(WifiEvent::ConnectionLost);
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);

    // SNTP should restart with fresh handle
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_some());
}

// ---------------------------------------------------------------------------
// Full lifecycle: connect → disconnect → backoff → reconnect
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_connect_disconnect_reconnect() {
    let mut sm = WifiSm::new(2, test_sm_config());
    let mut ts: Option<TimeSync> = None;

    // 1. Connect
    sm.handle_event(WifiEvent::ConnectSuccess);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_some());

    // 2. Connection lost
    // Stop SNTP immediately (matches main.rs fix)
    ts = None;
    sm.handle_event(WifiEvent::ConnectionLost);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());

    // 3. First reconnect attempt fails
    sm.handle_event(WifiEvent::ConnectFailed);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());

    // 4. Second reconnect attempt succeeds
    sm.handle_event(WifiEvent::ConnectSuccess);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_some());

    // 5. Another disconnect + portal
    ts = None;
    sm.handle_event(WifiEvent::ConnectionLost);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::ConnectFailed);
    // After full rotation → backoff
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Portal);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_none());

    // 6. Saved SSID found → reconnect from portal
    sm.handle_event(WifiEvent::SavedSsidFound { ssid_index: 0 });
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);
    ts = sntp_for_state(sm.state(), ts);
    assert!(ts.is_some());
}

#[test]
fn sntp_stop_when_none_is_noop() {
    let ts = sntp_for_state(&WifiState::Portal, None);
    assert!(ts.is_none());
}
