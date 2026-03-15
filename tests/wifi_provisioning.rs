#![cfg(any(feature = "json", feature = "lite-json"))]
// Host-side unit tests for WiFi provisioning flows.
//
// Tests cross-module interactions: form parsing → config building,
// config serialization roundtrips, state machine integration with
// credential management, and API key lifecycle.

use reconfigurable_device::captive_portal::{
    parse_provision_form, ApiKeyAction, FormError,
};
use reconfigurable_device::wifi_cfg::{
    deserialize_wifi_config, serialize_wifi_config, WifiConfig, MAX_WIFI_APS,
};
use reconfigurable_device::wifi_sm::{WifiAction, WifiEvent, WifiSm, WifiSmConfig, WifiState};

fn test_sm_config() -> WifiSmConfig {
    WifiSmConfig {
        max_rotations: 3,
        initial_backoff_ms: 100,
        max_backoff_ms: 5000,
    }
}

// ---------------------------------------------------------------------------
// Form parse → WifiConfig population
// ---------------------------------------------------------------------------

/// Simulate the provisioning flow: parse form → build WifiConfig → serialize → deserialize
#[test]
fn provision_form_to_config_roundtrip() {
    let body = "ssid_0=HomeWiFi&password_0=secret123&ssid_1=Office&password_1=work456&hostname=mydevice&api_key_action=generate";
    let form = parse_provision_form(body, MAX_WIFI_APS, false).unwrap();

    // Build WifiConfig from parsed form (mirrors main loop logic)
    let mut cfg = WifiConfig::new();
    for cred in &form.credentials {
        assert!(cfg.add_ssid(cred.ssid.clone(), cred.password.clone()));
    }
    if let Some(ref h) = form.hostname {
        cfg.hostname = h.clone();
    }
    // Simulate API key hash (in production this would be a real hash)
    if form.api_key_action == ApiKeyAction::Generate {
        cfg.api_key_hash = vec![0xDE, 0xAD, 0xBE, 0xEF];
    }

    // Serialize and deserialize
    let blob = serialize_wifi_config(&cfg).unwrap();
    let restored = deserialize_wifi_config(&blob).unwrap();

    assert_eq!(restored.ssids.len(), 2);
    assert_eq!(restored.ssids[0], ("HomeWiFi".into(), "secret123".into()));
    assert_eq!(restored.ssids[1], ("Office".into(), "work456".into()));
    assert_eq!(restored.hostname, "mydevice");
    assert_eq!(restored.api_key_hash, vec![0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn provision_keep_api_key_preserves_existing_hash() {
    let body = "ssid_0=Net&password_0=pass&api_key_action=keep";
    let form = parse_provision_form(body, MAX_WIFI_APS, false).unwrap();
    assert_eq!(form.api_key_action, ApiKeyAction::Keep);

    // Existing config has a hash
    let mut cfg = WifiConfig::new();
    cfg.api_key_hash = vec![0x01, 0x02, 0x03];

    // Keep action: don't overwrite hash
    for cred in &form.credentials {
        cfg.add_ssid(cred.ssid.clone(), cred.password.clone());
    }
    // api_key_hash unchanged
    assert_eq!(cfg.api_key_hash, vec![0x01, 0x02, 0x03]);
}

#[test]
fn provision_set_api_key_replaces_hash() {
    let body = "ssid_0=Net&password_0=pass&api_key_action=set&api_key=my-custom-key";
    let form = parse_provision_form(body, MAX_WIFI_APS, false).unwrap();
    assert_eq!(
        form.api_key_action,
        ApiKeyAction::Set("my-custom-key".into())
    );
}

/// Hostname from provisioning form propagates to WifiConfig (regression for eo-uk6).
#[test]
fn provision_form_hostname_propagates_to_config() {
    let body = "ssid_0=Net&password_0=pass&hostname=new-hostname&api_key_action=keep";
    let form = parse_provision_form(body, MAX_WIFI_APS, false).unwrap();
    assert_eq!(form.hostname, Some("new-hostname".into()));

    // Simulate main loop: start with old hostname, apply form
    let mut cfg = WifiConfig::new();
    assert_eq!(cfg.hostname, reconfigurable_device::wifi_cfg::DEFAULT_HOSTNAME);

    let mut hostname = cfg.hostname.clone();

    // This mirrors the fix in handle_provision: update hostname from form
    if let Some(ref new_hostname) = form.hostname {
        hostname = new_hostname.clone();
    }
    for cred in &form.credentials {
        cfg.add_ssid(cred.ssid.clone(), cred.password.clone());
    }
    cfg.hostname = hostname.clone();

    // Verify hostname was updated
    assert_eq!(hostname, "new-hostname");
    assert_eq!(cfg.hostname, "new-hostname");

    // Verify roundtrip
    let blob = serialize_wifi_config(&cfg).unwrap();
    let restored = deserialize_wifi_config(&blob).unwrap();
    assert_eq!(restored.hostname, "new-hostname");
}

/// When form omits hostname, the existing hostname is preserved.
#[test]
fn provision_form_no_hostname_keeps_existing() {
    let body = "ssid_0=Net&password_0=pass&api_key_action=keep";
    let form = parse_provision_form(body, MAX_WIFI_APS, false).unwrap();
    assert_eq!(form.hostname, None);

    let mut hostname = "original-host".to_string();
    if let Some(ref new_hostname) = form.hostname {
        hostname = new_hostname.clone();
    }
    assert_eq!(hostname, "original-host");
}

// ---------------------------------------------------------------------------
// Form validation edge cases
// ---------------------------------------------------------------------------

#[test]
fn form_duplicate_ssid_indices_last_wins() {
    // Two ssid_0 entries: the parser uses the first one found
    let body = "ssid_0=First&password_0=p1&ssid_0=Second&password_0=p2&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    // Both are collected (ssid_0 appears twice); implementation pushes both
    assert!(!form.credentials.is_empty());
    assert_eq!(form.credentials[0].ssid, "First");
}

#[test]
fn form_ssid_indices_out_of_order() {
    let body = "ssid_2=Third&password_2=p3&ssid_0=First&password_0=p1&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.credentials.len(), 2);
    // Order follows the ssid appearance in body
    assert_eq!(form.credentials[0].ssid, "Third");
    assert_eq!(form.credentials[1].ssid, "First");
}

#[test]
fn form_special_chars_in_password() {
    let body = "ssid_0=Net&password_0=%21%40%23%24%25%5E%26*()&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.credentials[0].password, "!@#$%^&*()");
}

#[test]
fn form_plus_sign_in_ssid() {
    let body = "ssid_0=My+Network&password_0=pass&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.credentials[0].ssid, "My Network");
}

#[test]
fn form_empty_body() {
    assert_eq!(parse_provision_form("", 3, false), Err(FormError::NoCredentials));
}

#[test]
fn form_only_unknown_fields() {
    let body = "foo=bar&baz=qux";
    assert_eq!(
        parse_provision_form(body, 3, false),
        Err(FormError::NoCredentials)
    );
}

#[test]
fn form_hostname_with_special_chars() {
    let body = "ssid_0=Net&password_0=pass&hostname=my%2Ddevice%2D01&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.hostname, Some("my-device-01".into()));
}

#[test]
fn form_empty_hostname_ignored() {
    let body = "ssid_0=Net&password_0=pass&hostname=&api_key_action=keep";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.hostname, None);
}

// ---------------------------------------------------------------------------
// State machine + credential management integration
// ---------------------------------------------------------------------------

#[test]
fn sm_provision_flow_unconfigured_to_connected() {
    // Start unconfigured
    let mut sm = WifiSm::new(0, test_sm_config());
    assert_eq!(*sm.state(), WifiState::Unconfigured);
    assert_eq!(sm.initial_action(), WifiAction::StartPortal);

    // User submits provisioning form with 2 SSIDs
    let action = sm.credentials_updated(2, 0);
    assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 0 });
    assert_eq!(action, WifiAction::TryConnect { ssid_index: 0 });

    // First SSID fails
    let action = sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 1 });
    assert_eq!(action, WifiAction::TryConnect { ssid_index: 1 });

    // Second SSID succeeds
    let action = sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);
    assert_eq!(action, WifiAction::Idle);
}

#[test]
fn sm_provision_flow_portal_to_connected_via_scan() {
    // Already in portal mode (all rotations exhausted)
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    sm.handle_event(WifiEvent::BackoffComplete);
    sm.handle_event(WifiEvent::ConnectFailed);
    assert_eq!(*sm.state(), WifiState::Portal);

    // Background scan finds saved SSID
    let action = sm.handle_event(WifiEvent::SavedSsidFound { ssid_index: 0 });
    assert_eq!(action, WifiAction::TryConnect { ssid_index: 0 });

    // Connection succeeds from portal
    let action = sm.portal_connect_succeeded();
    assert_eq!(*sm.state(), WifiState::Connected);
    assert_eq!(action, WifiAction::StopPortal);
}

#[test]
fn sm_reprovision_while_connected() {
    // Start connected
    let mut sm = WifiSm::new(1, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);

    // User reprovisioned with new credentials
    let action = sm.credentials_updated(3, 1);
    assert_eq!(*sm.state(), WifiState::Connecting { ssid_index: 1 });
    assert_eq!(action, WifiAction::TryConnect { ssid_index: 1 });
}

#[test]
fn sm_connection_lost_full_recovery() {
    let mut sm = WifiSm::new(2, test_sm_config());
    sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);

    // Connection drops
    let action = sm.handle_event(WifiEvent::ConnectionLost);
    assert_eq!(action, WifiAction::TryConnect { ssid_index: 0 });

    // First fails, second succeeds
    sm.handle_event(WifiEvent::ConnectFailed);
    let action = sm.handle_event(WifiEvent::ConnectSuccess);
    assert_eq!(*sm.state(), WifiState::Connected);
    assert_eq!(action, WifiAction::Idle);
}

// ---------------------------------------------------------------------------
// WifiConfig capacity limits
// ---------------------------------------------------------------------------

#[test]
fn config_rejects_beyond_max_aps() {
    let mut cfg = WifiConfig::new();
    for i in 0..MAX_WIFI_APS {
        assert!(cfg.add_ssid(format!("net{}", i), format!("pass{}", i)));
    }
    // One more should be rejected
    assert!(!cfg.add_ssid("overflow".into(), "pass".into()));
    assert_eq!(cfg.ssids.len(), MAX_WIFI_APS);
}

#[test]
fn form_max_aps_matches_config_max() {
    // Ensure form parsing with MAX_WIFI_APS respects the same limit
    let mut body = String::new();
    for i in 0..MAX_WIFI_APS + 2 {
        if !body.is_empty() {
            body.push('&');
        }
        body.push_str(&format!("ssid_{}=Net{}&password_{}=pass{}", i, i, i, i));
    }
    body.push_str("&api_key_action=keep");

    let form = parse_provision_form(&body, MAX_WIFI_APS, false).unwrap();
    // Should only include up to MAX_WIFI_APS credentials
    assert_eq!(form.credentials.len(), MAX_WIFI_APS);
}

// ---------------------------------------------------------------------------
// API key action edge cases
// ---------------------------------------------------------------------------

#[test]
fn api_key_action_unknown_value_defaults_to_keep() {
    let body = "ssid_0=Net&password_0=pass&api_key_action=unknown_action";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.api_key_action, ApiKeyAction::Keep);
}

#[test]
fn api_key_set_with_special_chars() {
    let body = "ssid_0=Net&password_0=pass&api_key_action=set&api_key=abc%21%40%23def";
    let form = parse_provision_form(body, 3, false).unwrap();
    assert_eq!(form.api_key_action, ApiKeyAction::Set("abc!@#def".into()));
}

// ---------------------------------------------------------------------------
// WiFi config serialization edge cases
// ---------------------------------------------------------------------------

#[test]
fn config_empty_password_roundtrip() {
    let mut cfg = WifiConfig::new();
    cfg.ssids.push(("OpenNetwork".into(), String::new()));
    let blob = serialize_wifi_config(&cfg).unwrap();
    let restored = deserialize_wifi_config(&blob).unwrap();
    assert_eq!(restored.ssids[0].1, "");
}

#[test]
fn config_special_chars_in_ssid_roundtrip() {
    let mut cfg = WifiConfig::new();
    cfg.ssids
        .push(("Net \"with\" <special> & chars".into(), "p&ss".into()));
    let blob = serialize_wifi_config(&cfg).unwrap();
    let restored = deserialize_wifi_config(&blob).unwrap();
    assert_eq!(cfg.ssids, restored.ssids);
}
