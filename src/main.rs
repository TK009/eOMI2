use anyhow::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::Peripherals,
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::{info, warn};
use reconfigurable_device::device::{
    build_sensor_tree, collect_writable_items, PATH_FREE_HEAP,
};
use reconfigurable_device::nvs::{load_writable_items, open_nvs, save_writable_items};
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::http::now_secs;
use reconfigurable_device::log_util::RateLimiter;
use reconfigurable_device::server::{dispatch_deliveries, start_http_server};
use reconfigurable_device::sync_util::lock_or_recover;
use reconfigurable_device::wifi_sm::{WifiSm, WifiSmConfig, WifiEvent, WifiAction, WifiState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: Option<&str> = option_env!("WIFI_PASS");
const API_TOKEN: Option<&str> = option_env!("API_TOKEN");

fn main() -> Result<()> {
    // Link ESP-IDF patches and initialize logging
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // Log level configuration:
    //   Release — default Info; noisy modules suppressed to Warn
    //   Debug   — default Debug; all modules at Debug unless overridden below
    //
    // Per-module targets (applied in both profiles):
    //   wifi, httpd, httpd_ws  — ESP-IDF C components, Warn only
    //   reconfigurable_device::omi — OMI protocol layer, Warn in release
    if cfg!(debug_assertions) {
        log::set_max_level(log::LevelFilter::Debug);
    } else {
        log::set_max_level(log::LevelFilter::Info);
    }

    // Quiet noisy ESP-IDF C components (both profiles)
    for target in &["wifi", "httpd", "httpd_ws"] {
        if let Err(e) = esp_idf_svc::log::set_target_level(target, log::LevelFilter::Warn) {
            warn!("Failed to set log level for '{}': {}", target, e);
        }
    }

    // Suppress verbose OMI protocol logging in release builds
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

    // Clone NVS partition for OMI tree persistence (Wi-Fi consumes the other)
    let nvs_omi = nvs.clone();

    // Build credential list: build-time creds first, then NVS-saved
    let nvs_wifi = nvs.clone();
    let wifi_cfg = reconfigurable_device::wifi_cfg::load_wifi_config_or_default(nvs_wifi);
    let mut creds: Vec<(String, String)> = Vec::new();

    if let (Some(s), Some(p)) = (WIFI_SSID, WIFI_PASS) {
        creds.push((s.to_string(), p.to_string()));
    }
    for (s, p) in &wifi_cfg.ssids {
        // Avoid duplicating build-time SSID if it's also in NVS
        if !creds.iter().any(|(existing, _)| existing == s) {
            creds.push((s.clone(), p.clone()));
        }
    }

    info!("WiFi credentials: {} available", creds.len());

    // Resolve API token: prefer build-time, fall back to NVS-stored hash presence check
    let api_token: &'static str = if let Some(t) = API_TOKEN {
        t
    } else {
        // Leak a placeholder — the captive portal flow (future) will handle runtime tokens
        anyhow::bail!("No API_TOKEN: set at build time or provision via captive portal");
    };

    // Initialize Wi-Fi driver
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    // Initialize WiFi state machine
    let sm_config = WifiSmConfig::default();
    let mut wifi_sm = WifiSm::new(creds.len(), sm_config);

    // Execute the initial action from the state machine
    let initial_action = wifi_sm.initial_action();
    execute_wifi_action(&mut wifi, &creds, &initial_action)?;

    // If we're already connected after initial action, log it
    if *wifi_sm.state() == WifiState::Connected {
        let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
        info!("Wi-Fi connected. IP: {}", ip_info.ip);
    } else if *wifi_sm.state() == WifiState::Unconfigured || *wifi_sm.state() == WifiState::Portal {
        // TODO: Start captive portal (eo-dnn: AP mode / soft-AP setup)
        anyhow::bail!("No WiFi credentials available and captive portal not yet implemented");
    }

    // Drive initial connection through the state machine
    if matches!(*wifi_sm.state(), WifiState::Connecting { .. }) {
        drive_initial_connect(&mut wifi_sm, &mut wifi, &creds)?;
    }

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected. IP: {}", ip_info.ip);

    // Dirty flag: set by HTTP handlers on successful writes, cleared by main loop after NVS save
    let nvs_dirty = Arc::new(AtomicBool::new(false));

    // Start HTTP server
    let (_server, engine, ws_senders, pending_deliveries) = start_http_server(nvs_dirty.clone(), api_token)?;
    info!("HTTP server listening on port 80");

    // Populate sensor tree
    {
        let mut eng = lock_or_recover(&engine, "engine");
        eng.tree.write_tree("/", build_sensor_tree()).unwrap();
        info!("Sensor tree populated: System/FreeHeap");
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

    // Main loop — read sensor, tick subscriptions, persist, keep Wi-Fi alive.
    // Sleep is split into short intervals so write-triggered event deliveries
    // (queued by HTTP handlers) are dispatched with low latency.
    const TICK_INTERVAL_MS: u64 = 5000;
    const POLL_INTERVAL_MS: u64 = 100;
    let mut elapsed_ms: u64 = TICK_INTERVAL_MS; // start with immediate first tick
    let mut wifi_rl = RateLimiter::new();
    let mut delivery_rl = RateLimiter::new();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
        elapsed_ms += POLL_INTERVAL_MS;

        // Drain write-triggered event deliveries queued by HTTP handlers.
        // Dispatched every POLL_INTERVAL_MS for low-latency event delivery.
        {
            let event_deliveries: Vec<_> = lock_or_recover(&pending_deliveries, "pending_deliveries")
                .drain(..)
                .collect();
            if !event_deliveries.is_empty() {
                dispatch_deliveries(&event_deliveries, &ws_senders, &engine, &mut delivery_rl);
            }
        }

        if elapsed_ms < TICK_INTERVAL_MS {
            continue;
        }
        elapsed_ms = 0;

        // WiFi reconnection via state machine
        if !wifi.is_connected()? {
            let action = wifi_sm.handle_event(WifiEvent::ConnectionLost);
            handle_reconnect_action(&mut wifi_sm, &mut wifi, &creds, action, &mut wifi_rl);
        }

        // Record free heap memory
        {
            // SAFETY: esp-idf C function with no preconditions; always safe to call.
            let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
            let now = now_secs();
            let mut eng = lock_or_recover(&engine, "engine");
            if let Err(e) = eng.tree.write_value(PATH_FREE_HEAP, OmiValue::Number(heap_free as f64), Some(now)) {
                warn!("Failed to write {}: {}", PATH_FREE_HEAP, e);
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

/// Drive the state machine through initial connection attempts until
/// connected or portal is needed.
fn drive_initial_connect(
    sm: &mut WifiSm,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
) -> Result<()> {
    loop {
        match sm.state() {
            WifiState::Connected => return Ok(()),
            WifiState::Portal | WifiState::Unconfigured => {
                // TODO: Start captive portal (eo-dnn: AP mode / soft-AP setup)
                anyhow::bail!(
                    "All WiFi credentials exhausted and captive portal not yet implemented"
                );
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
                // The SM told us to wait — sleep briefly during boot
                info!("WiFi backoff: waiting before next rotation");
                std::thread::sleep(std::time::Duration::from_millis(2000));
                sm.handle_event(WifiEvent::BackoffComplete);
            }
        }
    }
}

/// Execute a single WiFi action (for initial setup).
fn execute_wifi_action(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
    action: &WifiAction,
) -> Result<()> {
    match action {
        WifiAction::TryConnect { ssid_index } => {
            // Connection will be driven by drive_initial_connect
            Ok(())
        }
        WifiAction::StartPortal => {
            // Portal not yet implemented (eo-dnn)
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Handle reconnection in the main loop using the state machine.
fn handle_reconnect_action(
    sm: &mut WifiSm,
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    creds: &[(String, String)],
    mut action: WifiAction,
    rl: &mut RateLimiter,
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
            WifiAction::WaitBackoff { .. } | WifiAction::StartPortal | WifiAction::StopPortal => {
                // During main loop, don't block on backoff — just return and
                // let the next tick handle it. Portal actions are TODO (eo-dnn).
                break;
            }
            WifiAction::Idle => break,
        }
    }
}

fn try_connect(wifi: &mut BlockingWifi<EspWifi<'static>>, ssid: &str, pass: &str) -> Result<()> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: pass.try_into().map_err(|_| anyhow::anyhow!("Password too long"))?,
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    Ok(())
}
