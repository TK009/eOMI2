use anyhow::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::Peripherals,
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
    ws::FrameType,
};
use log::{info, warn};
use reconfigurable_device::device::{
    build_sensor_tree, collect_writable_items, PATH_FREE_HEAP,
};
use reconfigurable_device::nvs::{load_writable_items, open_nvs, save_writable_items};
use reconfigurable_device::odf::OmiValue;
use reconfigurable_device::omi::{Delivery, Engine, OmiResponse};
use reconfigurable_device::omi::subscriptions::DeliveryTarget;
use reconfigurable_device::omi::SessionId;
use reconfigurable_device::http::now_secs;
use reconfigurable_device::server::{start_http_server, WsSenders};
use reconfigurable_device::sync_util::lock_or_recover;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
const API_TOKEN: &str = env!("API_TOKEN");
const _: () = assert!(API_TOKEN.len() > 0, "API_TOKEN must not be empty");

fn main() -> Result<()> {
    // Link ESP-IDF patches and initialize logging
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

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

    // Connect to Wi-Fi
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;
    connect_wifi(&mut wifi)?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected. IP: {}", ip_info.ip);

    // Dirty flag: set by HTTP handlers on successful writes, cleared by main loop after NVS save
    let nvs_dirty = Arc::new(AtomicBool::new(false));

    // Start HTTP server
    let (_server, engine, ws_senders, pending_deliveries) = start_http_server(nvs_dirty.clone(), API_TOKEN)?;
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
                dispatch_deliveries(&event_deliveries, &ws_senders, &engine);
            }
        }

        if elapsed_ms < TICK_INTERVAL_MS {
            continue;
        }
        elapsed_ms = 0;

        if !wifi.is_connected()? {
            warn!("Wi-Fi disconnected, reconnecting...");
            connect_wifi(&mut wifi)?;
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
            dispatch_deliveries(&tick_deliveries, &ws_senders, &engine);
        }

        // Persist writable items to NVS if dirty
        if nvs_dirty.swap(false, Ordering::Acquire) {
            let eng = lock_or_recover(&engine, "engine");
            let items = collect_writable_items(&eng.tree);
            save_writable_items(&mut nvs_store, &items);
        }
    }
}

/// Dispatch subscription deliveries to WebSocket subscribers.
/// Removes failed senders and cancels their subscriptions.
fn dispatch_deliveries(
    deliveries: &[Delivery],
    ws_senders: &WsSenders,
    engine: &Arc<Mutex<Engine>>,
) {
    let mut failed_sessions: Vec<SessionId> = Vec::new();
    {
        let mut senders = lock_or_recover(ws_senders, "ws_senders");
        for d in deliveries {
            match &d.target {
                DeliveryTarget::WebSocket(session) => {
                    if let Some(sender) = senders.get_mut(session) {
                        let resp = OmiResponse::subscription_event(&d.rid, &d.path, &d.values);
                        match serde_json::to_string(&resp) {
                            Ok(json) => {
                                if sender.send(FrameType::Text(false), json.as_bytes()).is_err() {
                                    info!("WS send failed for session {}, removing", session);
                                    if !failed_sessions.contains(session) {
                                        failed_sessions.push(*session);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("WS delivery serialization failed: {}", e);
                            }
                        }
                    }
                }
                DeliveryTarget::Callback(_url) => {
                    info!(
                        "Sub delivery: rid={}, path={}, {} values (callback not yet implemented)",
                        d.rid, d.path, d.values.len()
                    );
                }
                DeliveryTarget::Poll => {} // handled via poll()
            }
        }
        for sid in &failed_sessions {
            senders.remove(sid);
        }
    }
    if !failed_sessions.is_empty() {
        let mut eng = lock_or_recover(engine, "engine");
        for sid in &failed_sessions {
            eng.subscriptions().cancel_by_ws_session(*sid);
        }
    }
}

fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Result<()> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: WIFI_PASS.try_into().map_err(|_| anyhow::anyhow!("Password too long"))?,
        ..Default::default()
    }))?;

    wifi.start()?;
    info!("Wi-Fi started, scanning...");

    wifi.connect()?;
    info!("Wi-Fi associated, waiting for IP...");

    wifi.wait_netif_up()?;
    Ok(())
}
