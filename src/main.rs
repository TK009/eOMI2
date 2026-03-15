use reconfigurable_device::error::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::Peripherals,
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::{info, warn};
use reconfigurable_device::boards;
use reconfigurable_device::captive_portal::{ConnectionState, ConnectionStatus};
use reconfigurable_device::device::{
    build_sensor_tree, collect_writable_items, update_discovery_tree,
    PATH_FREE_HEAP, PATH_TEMPERATURE,
};
#[cfg(feature = "mem-stats")]
use reconfigurable_device::device::{PATH_FREE_FLASH, PATH_FREE_ODF_STORAGE};
#[cfg(all(feature = "mem-stats", feature = "psram"))]
use reconfigurable_device::device::PATH_FREE_PSRAM;
use reconfigurable_device::dns::DnsServer;
use reconfigurable_device::mdns::{MdnsConfig, MdnsResponder};
use reconfigurable_device::time_sync::TimeSync;
use reconfigurable_device::nvs::{load_writable_items, open_nvs, save_writable_items};
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::http::now_secs;
use reconfigurable_device::log_util::RateLimiter;
use reconfigurable_device::server::{
    dispatch_deliveries, start_http_server, PortalState, ServerMode,
};
use reconfigurable_device::sync_util::lock_or_recover;
use reconfigurable_device::gpio::pwm::EdgeType;
use reconfigurable_device::wifi_ap;
use reconfigurable_device::wifi_cfg;
use reconfigurable_device::wifi_sm::{WifiSm, WifiSmConfig, WifiEvent, WifiAction, WifiState};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: Option<&str> = option_env!("WIFI_PASS");
const API_TOKEN: Option<&str> = option_env!("API_TOKEN");

fn main() -> Result<()> {
    // Link ESP-IDF patches and initialize logging
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    if cfg!(debug_assertions) {
        log::set_max_level(log::LevelFilter::Debug);
    } else {
        log::set_max_level(log::LevelFilter::Info);
    }

    for target in &["wifi", "httpd", "httpd_ws"] {
        if let Err(e) = esp_idf_svc::log::set_target_level(target, log::LevelFilter::Warn) {
            warn!("Failed to set log level for '{}': {}", target, e);
        }
    }

    if !cfg!(debug_assertions) {
        if let Err(e) = esp_idf_svc::log::set_target_level(
            "reconfigurable_device::omi",
            log::LevelFilter::Warn,
        ) {
            warn!("Failed to set log level for 'reconfigurable_device::omi': {}", e);
        }
    }

    info!("\n\n========================================");
    info!("  Reconfigurable Device v0.1.0");
    info!("  Serial port OK!");
    info!("========================================\n");
    info!("Reconfigurable device starting...");

    // Initialize peripherals
    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // Initialize GPIO and peripheral managers from board config (FR-001, FR-004, FR-013).
    // The boards module handles digital/edge pins (via AnyIOPin), PWM (LEDC),
    // ADC, and peripheral buses (I2C, UART) using typed HAL resources.
    let board_init = boards::init_board(peripherals, env!("EOMI_HOSTNAME"))?;
    let mut gpio_manager = board_init.gpio_manager;
    let mut peripheral_manager = board_init.peripheral_manager;

    let nvs_omi = nvs.clone();
    let nvs_wifi_cfg = nvs.clone();
    let nvs_wifi_persist = nvs.clone();

    // Load WiFi configuration from NVS
    let mut wifi_cfg = wifi_cfg::load_wifi_config_or_default(nvs_wifi_cfg);

    // Keep a writable NVS handle for persisting WiFi config after provisioning (SC-004)
    let mut wifi_nvs = wifi_cfg::open_wifi_cfg_nvs(nvs_wifi_persist)?;
    let mut creds: Vec<(String, String)> = Vec::new();

    if let (Some(s), Some(p)) = (WIFI_SSID, WIFI_PASS) {
        creds.push((s.to_string(), p.to_string()));
    }
    for (s, p) in &wifi_cfg.ssids {
        if !creds.iter().any(|(existing, _)| existing == s) {
            creds.push((s.clone(), p.clone()));
        }
    }

    info!("WiFi credentials: {} available", creds.len());
    let mut hostname = wifi_cfg.hostname.clone();

    // Resolve API token: use build-time value if set, otherwise empty placeholder
    // so the device can boot into captive portal for first-time provisioning.
    // With an empty token, check_bearer_auth denies all mutating API requests
    // until a proper token is configured and the device reboots.
    let api_token: &'static str = match API_TOKEN {
        Some(t) => t,
        None => {
            warn!("No API_TOKEN set at build time — API auth disabled until provisioned");
            ""
        }
    };

    // Initialize Wi-Fi driver (modem returned from board init)
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(board_init.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    // Initialize WiFi state machine
    let sm_config = WifiSmConfig::default();
    let mut wifi_sm = WifiSm::new(creds.len(), sm_config);

    let mut ap_active = false;
    let mut dns_server: Option<DnsServer> = None;
    let mut mdns_responder: Option<MdnsResponder> = None;
    let mut time_sync: Option<TimeSync> = None;
    let mut last_sta_ip: Option<String> = None;

    // Determine initial server mode based on WiFi state
    let initial_mode = if creds.is_empty() {
        ServerMode::Portal
    } else {
        ServerMode::Normal
    };

    // Execute the initial action from the state machine
    let initial_action = wifi_sm.initial_action();
    match &initial_action {
        WifiAction::StartPortal => {
            wifi_ap::start_ap(&mut wifi, &hostname)?;
            ap_active = true;
            dns_server = start_dns(wifi_ap::AP_IP);
            info!("Captive portal active — waiting for provisioning");
        }
        _ => {}
    }

    // Drive initial connection through the state machine
    if matches!(*wifi_sm.state(), WifiState::Connecting { .. }) {
        drive_initial_connect(&mut wifi_sm, &mut wifi, &creds, &hostname, &mut ap_active);
    }

    // If we ended up in Portal state, ensure AP + DNS are running
    if matches!(*wifi_sm.state(), WifiState::Portal | WifiState::Unconfigured) && !ap_active {
        wifi_ap::start_ap(&mut wifi, &hostname)?;
        ap_active = true;
        dns_server = start_dns(wifi_ap::AP_IP);
    }

    // Resolve current mode based on WiFi state
    let current_mode = if ap_active && !matches!(*wifi_sm.state(), WifiState::Connected) {
        ServerMode::Portal
    } else {
        initial_mode
    };

    if *wifi_sm.state() == WifiState::Connected {
        let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
        info!("Wi-Fi connected. IP: {}", ip_info.ip);
        last_sta_ip = Some(ip_info.ip.to_string());

        // FR-007: start mDNS only in station mode (not AP)
        match MdnsResponder::start(MdnsConfig::new(&hostname)) {
            Ok(resp) => {
                info!("mDNS responder started for {}.local", hostname);
                mdns_responder = Some(resp);
            }
            Err(e) => warn!("Failed to start mDNS responder: {}", e),
        }
    } else if ap_active {
        info!("No STA connection yet — captive portal active on {}", wifi_ap::AP_IP);
    }

    // Create portal state (shared between HTTP handlers and main loop)
    let saved_ssids: Vec<String> = wifi_cfg.ssids.iter().map(|(s, _)| s.clone()).collect();
    let is_first_setup = wifi_cfg.api_key_hash.is_empty();
    let portal = Arc::new(PortalState::new(
        current_mode,
        wifi_cfg::MAX_WIFI_APS,
        saved_ssids,
        hostname.clone(),
        is_first_setup,
    ));

    // Dirty flag for NVS persistence
    let nvs_dirty = Arc::new(AtomicBool::new(false));

    // Start HTTP server with portal state
    let (_server, engine, ws_senders, pending_deliveries) =
        start_http_server(nvs_dirty.clone(), api_token, portal.clone())?;
    info!("HTTP server listening on port 80");

    // FR-020: Confirm running firmware as valid to cancel OTA rollback.
    // Without this call, ESP-IDF's rollback mechanism reboots into the
    // previous OTA slot on watchdog timeout (FR-021).
    unsafe {
        let err = esp_idf_svc::sys::esp_ota_mark_app_valid_cancel_rollback();
        if err != esp_idf_svc::sys::ESP_OK {
            warn!("esp_ota_mark_app_valid_cancel_rollback failed: {}", err);
        } else {
            info!("OTA: running firmware confirmed valid (rollback cancelled)");
        }
    }

    // Populate sensor tree
    {
        let mut eng = lock_or_recover(&engine, "engine");
        eng.tree.write_tree("/", build_sensor_tree())?;
        info!("Sensor tree populated: System/{{FirmwareVersion,FreeHeap,FreeFlash,FreePsram,FreeOdfStorage,Temperature}}");

        // Register PWM InfoItems as writable entries in the O-DF tree (FR-004)
        gpio_manager.register_tree_items(&mut eng.tree);

        // Register UART/SPI RX/TX InfoItems in the O-DF tree (FR-008, FR-009)
        peripheral_manager.register_tree_items(&mut eng.tree);
    }

    // Load and replay NVS-persisted writable items
    let mut nvs_store = open_nvs(nvs_omi)?;
    {
        let saved_items = load_writable_items(&nvs_store);
        if !saved_items.is_empty() {
            let mut eng = lock_or_recover(&engine, "engine");
            for saved in &saved_items {
                if let Err(e) = eng.tree.write_value(&saved.path, saved.v.clone(), saved.t) {
                    warn!("Failed to restore {}: {}", saved.path, e);
                    continue;
                }
                eng.mark_writable(&saved.path);
            }
            info!("Restored {} writable items from NVS", saved_items.len());
        }
    }

    // Perform initial WiFi scan if portal is active (populate /scan results)
    if ap_active {
        match wifi_ap::scan_networks(&mut wifi) {
            Ok(networks) => {
                info!("Initial WiFi scan: {} networks found", networks.len());
                *lock_or_recover(&portal.scan_results, "scan_results") = networks;
            }
            Err(e) => warn!("Initial WiFi scan failed: {}", e),
        }
    }

    // Initialize temperature sensor if supported by this board
    let temp_sensor = reconfigurable_device::temp_sensor::TempSensor::new();
    if temp_sensor.is_some() {
        info!("Temperature sensor initialised");
    }

    // Query memory totals at boot and set meta.total on each memory InfoItem
    {
        use reconfigurable_device::odf::PathTargetMut;
        let mut eng = lock_or_recover(&engine, "engine");

        // FreeHeap total (always available)
        if let Some(stat) = reconfigurable_device::mem_stats::heap() {
            if let Ok(PathTargetMut::InfoItem(item)) = eng.tree.resolve_mut(PATH_FREE_HEAP) {
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("total".into(), OmiValue::Number(stat.total as f64));
            }
        }

        #[cfg(feature = "mem-stats")]
        {
            if let Some(stat) = reconfigurable_device::mem_stats::flash() {
                if let Ok(PathTargetMut::InfoItem(item)) = eng.tree.resolve_mut(PATH_FREE_FLASH) {
                    let meta = item.meta.get_or_insert_with(BTreeMap::new);
                    meta.insert("total".into(), OmiValue::Number(stat.total as f64));
                }
            }
            if let Some(stat) = reconfigurable_device::mem_stats::nvs() {
                if let Ok(PathTargetMut::InfoItem(item)) = eng.tree.resolve_mut(PATH_FREE_ODF_STORAGE) {
                    let meta = item.meta.get_or_insert_with(BTreeMap::new);
                    meta.insert("total".into(), OmiValue::Number(stat.total as f64));
                }
            }
        }

        #[cfg(all(feature = "mem-stats", feature = "psram"))]
        {
            if let Some(stat) = reconfigurable_device::mem_stats::psram() {
                if let Ok(PathTargetMut::InfoItem(item)) = eng.tree.resolve_mut(PATH_FREE_PSRAM) {
                    let meta = item.meta.get_or_insert_with(BTreeMap::new);
                    meta.insert("total".into(), OmiValue::Number(stat.total as f64));
                }
            }
        }
    }

    // Main loop
    const TICK_INTERVAL_MS: u64 = 5000;
    const POLL_INTERVAL_MS: u64 = 100;
    const SCAN_INTERVAL_MS: u64 = 15_000; // WiFi scan every 15s in portal mode
    const DISCOVERY_INTERVAL_MS: u64 = 30_000; // mDNS peer discovery every 30s
    #[cfg(feature = "mem-stats")]
    const MEMORY_INTERVAL_MS: u64 = 30_000; // Memory stats every 30s
    const TEMP_INTERVAL_MS: u64 = 300_000; // Temperature every 5min
    let mut elapsed_ms: u64 = TICK_INTERVAL_MS;
    let mut scan_elapsed_ms: u64 = 0;
    let mut discovery_elapsed_ms: u64 = DISCOVERY_INTERVAL_MS; // trigger on first tick
    #[cfg(feature = "mem-stats")]
    let mut memory_elapsed_ms: u64 = MEMORY_INTERVAL_MS; // trigger on first tick
    let mut temp_elapsed_ms: u64 = TEMP_INTERVAL_MS; // trigger on first tick
    let mut backoff_deadline: Option<Instant> = None;
    let mut wifi_rl = RateLimiter::new();
    let mut delivery_rl = RateLimiter::new();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
        elapsed_ms += POLL_INTERVAL_MS;
        scan_elapsed_ms += POLL_INTERVAL_MS;

        // Drain write-triggered event deliveries (100ms cadence preserved)
        {
            let event_deliveries: Vec<_> = lock_or_recover(&pending_deliveries, "pending_deliveries")
                .drain(..)
                .collect();
            if !event_deliveries.is_empty() {
                dispatch_deliveries(&event_deliveries, &ws_senders, &engine, &mut delivery_rl);
            }
        }

        // Digital input polling: read all digital_in pins at 100ms cadence (FR-005, FR-007)
        if gpio_manager.has_digital_pins() {
            let now = now_secs();
            let mut eng = lock_or_recover(&engine, "engine");
            gpio_manager.poll_digital_inputs(&mut eng.tree, now);
        }

        // Edge trigger ISR: drain fired edge events and update O-DF tree (FR-005a)
        if gpio_manager.has_edge_pins() {
            let edge_events = gpio_manager.drain_edge_events();
            if !edge_events.is_empty() {
                let now = now_secs();
                let mut eng = lock_or_recover(&engine, "engine");
                for event in &edge_events {
                    let value = match event.edge_type {
                        EdgeType::Low => OmiValue::Number(0.0),
                        EdgeType::High => OmiValue::Number(1.0),
                    };
                    if let Err(e) = eng.tree.write_value(&event.path, value, Some(now)) {
                        warn!("Edge event write failed for {}: {}", event.path, e);
                    }
                }
            }
        }

        // PWM write actuation: sync pin outputs from O-DF tree values (FR-004)
        if gpio_manager.has_pwm_pins() {
            let eng = lock_or_recover(&engine, "engine");
            gpio_manager.sync_from_tree(&eng.tree);
        }

        // Digital output actuation: sync pin outputs from O-DF tree values (FR-004)
        if gpio_manager.has_digital_pins() {
            let eng = lock_or_recover(&engine, "engine");
            gpio_manager.sync_digital_outputs(&eng.tree);
        }

        // Peripheral bus polling: UART RX + TX sync, SPI transfers (FR-008, FR-009)
        if peripheral_manager.has_buses() {
            let now = now_secs();
            let mut eng = lock_or_recover(&engine, "engine");
            peripheral_manager.poll(&mut eng.tree, now);
        }

        // Check for pending provisioning request (FR-016)
        if let Some(provision) = portal.take_pending_provision() {
            handle_provision(
                &provision,
                &mut wifi,
                &mut wifi_sm,
                &mut creds,
                &mut hostname,
                &mut ap_active,
                &mut dns_server,
                &portal,
                &mut wifi_nvs,
                &mut wifi_cfg,
            );
            // Restart mDNS with updated hostname if connected
            if let Some(resp) = mdns_responder.take() {
                drop(resp);
                match MdnsResponder::start(MdnsConfig::new(&hostname)) {
                    Ok(resp) => {
                        info!("mDNS responder restarted for {}.local", hostname);
                        mdns_responder = Some(resp);
                    }
                    Err(e) => warn!("Failed to restart mDNS responder: {}", e),
                }
            }
        }

        // Check backoff timer completion
        if let Some(deadline) = backoff_deadline {
            if Instant::now() >= deadline {
                backoff_deadline = None;
                let action = wifi_sm.handle_event(WifiEvent::BackoffComplete);
                handle_reconnect_action(
                    &mut wifi_sm, &mut wifi, &creds, action, &mut wifi_rl,
                    &hostname, &mut ap_active, &mut dns_server, &portal,
                    &mut backoff_deadline,
                );
            }
        }

        // Periodic WiFi scan while portal is active (FR-018 + populate /scan)
        if ap_active && scan_elapsed_ms >= SCAN_INTERVAL_MS {
            scan_elapsed_ms = 0;
            match wifi_ap::scan_networks(&mut wifi) {
                Ok(networks) => {
                    // Check for saved SSIDs in scan results (FR-018 auto-reconnect)
                    if matches!(*wifi_sm.state(), WifiState::Portal | WifiState::Unconfigured) {
                        if let Some(idx) = find_saved_ssid_in_scan(&networks, &creds) {
                            info!("FR-018: saved SSID '{}' found in scan, attempting auto-reconnect", creds[idx].0);
                            let action = wifi_sm.handle_event(WifiEvent::SavedSsidFound { ssid_index: idx });
                            handle_portal_reconnect(
                                &mut wifi_sm, &mut wifi, &creds, action,
                                &hostname, &mut ap_active, &mut dns_server, &portal,
                            );
                        }
                    }
                    *lock_or_recover(&portal.scan_results, "scan_results") = networks;
                }
                Err(e) => warn!("WiFi scan failed: {}", e),
            }
        }

        if elapsed_ms < TICK_INTERVAL_MS {
            continue;
        }
        elapsed_ms = 0;

        // WiFi reconnection via state machine
        if !wifi.is_connected().unwrap_or(false) && *wifi_sm.state() == WifiState::Connected {
            // Stop SNTP immediately on disconnect — avoids leaking stale handles
            // across reconnection cycles and ensures a fresh sync on reconnect.
            if time_sync.take().is_some() {
                info!("SNTP time sync stopped (WiFi disconnected)");
            }

            let action = wifi_sm.handle_event(WifiEvent::ConnectionLost);
            handle_reconnect_action(
                &mut wifi_sm, &mut wifi, &creds, action, &mut wifi_rl,
                &hostname, &mut ap_active, &mut dns_server, &portal,
                &mut backoff_deadline,
            );
        }

        // Update portal mode based on WiFi state
        let target_mode = match wifi_sm.state() {
            WifiState::Connected => ServerMode::Normal,
            WifiState::Portal | WifiState::Unconfigured => ServerMode::Portal,
            _ => portal.mode(), // Keep current mode during transient states
        };
        if portal.mode() != target_mode {
            portal.set_mode(target_mode);
            info!("Server mode changed to {:?}", target_mode);
        }

        // mDNS lifecycle: start when Connected, stop otherwise (FR-007)
        match wifi_sm.state() {
            WifiState::Connected => {
                if mdns_responder.is_none() {
                    match MdnsResponder::start(MdnsConfig::new(&hostname)) {
                        Ok(resp) => {
                            info!("mDNS responder started for {}.local", hostname);
                            mdns_responder = Some(resp);
                        }
                        Err(e) => warn!("Failed to start mDNS responder: {}", e),
                    }
                }

                // Start SNTP time sync after mDNS (non-blocking, syncs in background)
                if time_sync.is_none() {
                    time_sync = TimeSync::start();
                }

                // Check for IP change (DHCP renewal)
                if let Ok(ip_info) = wifi.wifi().sta_netif().get_ip_info() {
                    let current_ip = ip_info.ip.to_string();
                    if last_sta_ip.as_deref() != Some(&current_ip) {
                        info!("STA IP changed: {:?} → {}", last_sta_ip, current_ip);
                        last_sta_ip = Some(current_ip);
                        if let Some(ref mut resp) = mdns_responder {
                            if let Err(e) = resp.update_ip() {
                                warn!("mDNS update_ip failed: {}", e);
                            }
                        }
                    }
                }
            }
            _ => {
                // FR-007: mDNS MUST NOT be active in AP mode or when disconnected
                if let Some(resp) = mdns_responder.take() {
                    info!("Stopping mDNS responder (state: {:?})", wifi_sm.state());
                    resp.stop();
                    last_sta_ip = None;
                }
                // Stop SNTP when WiFi disconnects (no network for time sync)
                if time_sync.take().is_some() {
                    info!("SNTP time sync stopped (WiFi disconnected)");
                }
            }
        }

        // FR-009: Periodic mDNS peer discovery (every 30s when connected)
        discovery_elapsed_ms += TICK_INTERVAL_MS;
        if discovery_elapsed_ms >= DISCOVERY_INTERVAL_MS {
            discovery_elapsed_ms = 0;
            if let Some(ref resp) = mdns_responder {
                let peers = reconfigurable_device::mdns_discovery::discover_peers(resp);
                let now = now_secs();
                let mut eng = lock_or_recover(&engine, "engine");
                let removed = update_discovery_tree(&mut eng.tree, &peers, Some(now));
                if !peers.is_empty() || removed > 0 {
                    info!("Discovery: {} peers, {} stale removed", peers.len(), removed);
                }
            }
        }

        // ADC sampling: read all analog_in pins at tick interval (FR-005, FR-007)
        if gpio_manager.has_adc_pins() {
            let now = now_secs();
            let mut eng = lock_or_recover(&engine, "engine");
            gpio_manager.sample_adc(&mut eng.tree, now);
        }

        // Record free heap memory (5s tick)
        if let Some(stat) = reconfigurable_device::mem_stats::heap() {
            let now = now_secs();
            let mut eng = lock_or_recover(&engine, "engine");
            if let Err(e) = eng.tree.write_value(PATH_FREE_HEAP, OmiValue::Number(stat.free as f64), Some(now)) {
                warn!("Failed to write {}: {}", PATH_FREE_HEAP, e);
            }
        }

        // Record memory stats (30s interval): FreeFlash, FreePsram, FreeOdfStorage
        #[cfg(feature = "mem-stats")]
        {
            memory_elapsed_ms += TICK_INTERVAL_MS;
            if memory_elapsed_ms >= MEMORY_INTERVAL_MS {
                memory_elapsed_ms = 0;
                let now = now_secs();
                let mut eng = lock_or_recover(&engine, "engine");
                if let Some(stat) = reconfigurable_device::mem_stats::flash() {
                    if let Err(e) = eng.tree.write_value(PATH_FREE_FLASH, OmiValue::Number(stat.free as f64), Some(now)) {
                        warn!("Failed to write {}: {}", PATH_FREE_FLASH, e);
                    }
                }
                #[cfg(feature = "psram")]
                if let Some(stat) = reconfigurable_device::mem_stats::psram() {
                    if let Err(e) = eng.tree.write_value(PATH_FREE_PSRAM, OmiValue::Number(stat.free as f64), Some(now)) {
                        warn!("Failed to write {}: {}", PATH_FREE_PSRAM, e);
                    }
                }
                if let Some(stat) = reconfigurable_device::mem_stats::nvs() {
                    if let Err(e) = eng.tree.write_value(PATH_FREE_ODF_STORAGE, OmiValue::Number(stat.free as f64), Some(now)) {
                        warn!("Failed to write {}: {}", PATH_FREE_ODF_STORAGE, e);
                    }
                }
            }
        }

        // Record temperature (5min interval)
        temp_elapsed_ms += TICK_INTERVAL_MS;
        if temp_elapsed_ms >= TEMP_INTERVAL_MS {
            temp_elapsed_ms = 0;
            if let Some(ref ts) = temp_sensor {
                if let Some(celsius) = ts.read_celsius() {
                    let now = now_secs();
                    let mut eng = lock_or_recover(&engine, "engine");
                    if let Err(e) = eng.tree.write_value(PATH_TEMPERATURE, OmiValue::Number(celsius), Some(now)) {
                        warn!("Failed to write {}: {}", PATH_TEMPERATURE, e);
                    }
                }
            }
        }

        // Tick interval subscriptions and dispatch
        let tick_deliveries = {
            let mut eng = lock_or_recover(&engine, "engine");
            eng.tick(now_secs())
        };
        if !tick_deliveries.is_empty() {
            dispatch_deliveries(&tick_deliveries, &ws_senders, &engine, &mut delivery_rl);
        }

        // Persist writable items to NVS if dirty
        if nvs_dirty.swap(false, Ordering::Acquire) {
            let eng = lock_or_recover(&engine, "engine");
            let items = collect_writable_items(&eng.tree);
            save_writable_items(&mut nvs_store, &items);
        }
    }
}

/// Start DNS server for captive portal redirect, logging errors.
fn start_dns(redirect_ip: &str) -> Option<DnsServer> {
    match DnsServer::start("0.0.0.0", redirect_ip) {
        Ok(dns) => {
            info!("DNS responder started, redirecting to {}", redirect_ip);
            Some(dns)
        }
        Err(e) => {
            warn!("Failed to start DNS responder: {}", e);
            None
        }
    }
}

/// Handle a provisioning request from the captive portal form (FR-016).
fn handle_provision(
    provision: &reconfigurable_device::server::PendingProvision,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_sm: &mut WifiSm,
    creds: &mut Vec<(String, String)>,
    hostname: &mut String,
    ap_active: &mut bool,
    dns_server: &mut Option<DnsServer>,
    portal: &Arc<PortalState>,
    wifi_nvs: &mut esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>,
    wifi_cfg: &mut wifi_cfg::WifiConfig,
) {
    let form = &provision.form;

    // Update hostname from form if provided (fixes hostname propagation bug)
    if let Some(ref new_hostname) = form.hostname {
        *hostname = new_hostname.clone();
    }

    // Hash and store the API key (FR-005)
    use reconfigurable_device::captive_portal::ApiKeyAction;
    match &form.api_key_action {
        ApiKeyAction::Generate => {
            if let Some(ref key) = provision.generated_api_key {
                wifi_cfg.set_api_key(key);
                info!("Provisioning: API key generated and hashed");
            }
        }
        ApiKeyAction::Set(key) => {
            wifi_cfg.set_api_key(key);
            info!("Provisioning: user-provided API key hashed");
        }
        ApiKeyAction::Keep => {}
    }

    // Add new credentials
    let start_index = creds.len();
    for cred in &form.credentials {
        if cred.password.is_empty() {
            warn!("Provisioning: empty password for SSID '{}' — may fail on non-open networks", cred.ssid);
        }
        // Check for duplicates
        if !creds.iter().any(|(s, _)| s == &cred.ssid) {
            creds.push((cred.ssid.clone(), cred.password.clone()));
        } else {
            // Update password for existing SSID
            if let Some(existing) = creds.iter_mut().find(|(s, _)| s == &cred.ssid) {
                if !cred.password.is_empty() {
                    existing.1 = cred.password.clone();
                }
            }
        }
    }

    info!("Provisioning: {} total credentials after update", creds.len());

    // Persist updated config (credentials + hostname + API key hash) to NVS (FR-005, FR-013, SC-004, SC-007)
    wifi_cfg.ssids = creds.clone();
    wifi_cfg.hostname = hostname.clone();
    if !wifi_cfg::save_wifi_config(wifi_nvs, wifi_cfg) {
        warn!("Provisioning: failed to persist WiFi config to NVS");
    }

    // Update portal form config so re-renders show the new SSIDs/hostname (SC-004)
    let saved_ssids: Vec<String> = creds.iter().map(|(s, _)| s.clone()).collect();
    portal.update_form_config(saved_ssids, hostname.clone());

    // Notify state machine of new credentials
    let action = wifi_sm.credentials_updated(creds.len(), start_index);

    // Try connecting with the new credentials using AP+STA mixed mode
    match action {
        WifiAction::TryConnect { ssid_index } => {
            if ssid_index < creds.len() {
                let (ssid, pass) = &creds[ssid_index];
                info!("Provisioning: attempting connection to {}", ssid);
                match wifi_ap::try_connect_sta(wifi, ssid, pass, hostname) {
                    Ok(()) => {
                        info!("Provisioning: connected to {}", ssid);
                        wifi_sm.handle_event(WifiEvent::ConnectSuccess);

                        let ip_info = wifi.wifi().sta_netif().get_ip_info()
                            .map(|i| i.ip.to_string())
                            .unwrap_or_default();

                        *lock_or_recover(&portal.connection_status, "connection_status") =
                            ConnectionStatus {
                                state: ConnectionState::Connected,
                                message: None,
                                ip: Some(ip_info),
                            };

                        // Transition to normal mode: stop DNS (AP stays briefly for success page)
                        if let Some(dns) = dns_server.take() {
                            drop(dns);
                        }
                        portal.set_mode(ServerMode::Normal);
                    }
                    Err(e) => {
                        warn!("Provisioning: connection to {} failed: {}", ssid, e);
                        wifi_sm.handle_event(WifiEvent::ConnectFailed);
                        *lock_or_recover(&portal.connection_status, "connection_status") =
                            ConnectionStatus {
                                state: ConnectionState::Failed,
                                message: Some(format!("Connection failed: {}", e)),
                                ip: None,
                            };
                        portal.set_form_error(Some(format!("Connection to {} failed: {}", ssid, e)));
                    }
                }
            }
        }
        _ => {
            warn!("Provisioning: unexpected action from state machine: {:?}", action);
        }
    }
}

/// Drive the state machine through initial connection attempts.
fn drive_initial_connect(
    sm: &mut WifiSm,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
    hostname: &str,
    ap_active: &mut bool,
) {
    loop {
        match sm.state() {
            WifiState::Connected => return,
            WifiState::Portal | WifiState::Unconfigured => {
                if !*ap_active {
                    if let Err(e) = wifi_ap::start_ap(wifi, hostname) {
                        warn!("Failed to start soft-AP: {}", e);
                    } else {
                        *ap_active = true;
                    }
                }
                info!("Captive portal active — waiting for provisioning");
                return;
            }
            WifiState::Connecting { ssid_index } => {
                let idx = *ssid_index;
                let (ssid, pass) = &creds[idx];
                info!("Trying WiFi SSID [{}]: {}", idx, ssid);
                let event = match try_connect(wifi, ssid, pass) {
                    Ok(()) => {
                        info!("WiFi connected to {}", ssid);
                        WifiEvent::ConnectSuccess
                    }
                    Err(e) => {
                        warn!("WiFi connect to {} failed: {}", ssid, e);
                        WifiEvent::ConnectFailed
                    }
                };
                sm.handle_event(event);
            }
            WifiState::Backoff => {
                let ms = sm.backoff_ms();
                info!("WiFi backoff: {}ms before next rotation", ms);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                sm.handle_event(WifiEvent::BackoffComplete);
            }
        }
    }
}

/// Handle reconnection in the main loop using the state machine.
fn handle_reconnect_action(
    sm: &mut WifiSm,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
    mut action: WifiAction,
    rl: &mut RateLimiter,
    hostname: &str,
    ap_active: &mut bool,
    dns_server: &mut Option<DnsServer>,
    portal: &Arc<PortalState>,
    backoff_deadline: &mut Option<Instant>,
) {
    loop {
        match action {
            WifiAction::TryConnect { ssid_index } => {
                if ssid_index >= creds.len() {
                    break;
                }
                let (ssid, pass) = &creds[ssid_index];
                let event = match try_connect(wifi, ssid, pass) {
                    Ok(()) => {
                        info!("Wi-Fi reconnected to {}", ssid);
                        WifiEvent::ConnectSuccess
                    }
                    Err(e) => {
                        let msg = format!("Wi-Fi reconnect to {} failed: {}", ssid, e);
                        if rl.should_emit(&msg) {
                            warn!("{}", msg);
                        }
                        WifiEvent::ConnectFailed
                    }
                };
                action = sm.handle_event(event);
            }
            WifiAction::StartPortal => {
                if !*ap_active {
                    if let Err(e) = wifi_ap::start_ap(wifi, hostname) {
                        warn!("Failed to start soft-AP: {}", e);
                    } else {
                        *ap_active = true;
                        *dns_server = start_dns(wifi_ap::AP_IP);
                        portal.set_mode(ServerMode::Portal);
                        info!("Captive portal started after connection loss");
                    }
                }
                break;
            }
            WifiAction::StopPortal => {
                if let Some(dns) = dns_server.take() {
                    drop(dns);
                }
                if *ap_active {
                    if let Err(e) = wifi_ap::stop_ap(wifi) {
                        warn!("Failed to stop soft-AP: {}", e);
                    } else {
                        *ap_active = false;
                    }
                }
                portal.set_mode(ServerMode::Normal);
                break;
            }
            WifiAction::WaitBackoff { ms } => {
                info!("WiFi backoff: {}ms before next rotation", ms);
                *backoff_deadline = Some(Instant::now() + std::time::Duration::from_millis(ms));
                break;
            }
            WifiAction::Idle => break,
        }
    }
}

/// Find the first saved SSID that appears in scan results (FR-018).
fn find_saved_ssid_in_scan(
    networks: &[reconfigurable_device::captive_portal::ScannedNetwork],
    creds: &[(String, String)],
) -> Option<usize> {
    for (idx, (ssid, _)) in creds.iter().enumerate() {
        if networks.iter().any(|n| n.ssid == *ssid) {
            return Some(idx);
        }
    }
    None
}

/// Handle auto-reconnect attempt from portal when a saved SSID is found (FR-018).
fn handle_portal_reconnect(
    sm: &mut WifiSm,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
    action: WifiAction,
    hostname: &str,
    ap_active: &mut bool,
    dns_server: &mut Option<DnsServer>,
    portal: &Arc<PortalState>,
) {
    if let WifiAction::TryConnect { ssid_index } = action {
        if ssid_index < creds.len() {
            let (ssid, pass) = &creds[ssid_index];
            match wifi_ap::try_connect_sta(wifi, ssid, pass, hostname) {
                Ok(()) => {
                    info!("FR-018: auto-reconnected to {}", ssid);
                    let stop_action = sm.portal_connect_succeeded();

                    // Stop portal infrastructure
                    if matches!(stop_action, WifiAction::StopPortal) {
                        if let Some(dns) = dns_server.take() {
                            drop(dns);
                        }
                        if *ap_active {
                            if let Err(e) = wifi_ap::stop_ap(wifi) {
                                warn!("Failed to stop soft-AP: {}", e);
                            } else {
                                *ap_active = false;
                            }
                        }
                        portal.set_mode(ServerMode::Normal);

                        let ip_info = wifi.wifi().sta_netif().get_ip_info()
                            .map(|i| i.ip.to_string())
                            .unwrap_or_default();
                        info!("Wi-Fi connected. IP: {}", ip_info);
                    }
                }
                Err(e) => {
                    warn!("FR-018: auto-reconnect to {} failed: {}", ssid, e);
                    sm.handle_event(WifiEvent::ConnectFailed);
                }
            }
        }
    }
}

fn try_connect(wifi: &mut BlockingWifi<EspWifi<'static>>, ssid: &str, pass: &str) -> Result<()> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| reconfigurable_device::error::Error::Msg("SSID too long"))?,
        password: pass.try_into().map_err(|_| reconfigurable_device::error::Error::Msg("Password too long"))?,
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    Ok(())
}
