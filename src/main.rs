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
use reconfigurable_device::server::{dispatch_deliveries, start_http_server};
use reconfigurable_device::sync_util::lock_or_recover;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
const API_TOKEN: &str = env!("API_TOKEN");
const _: () = assert!(API_TOKEN.len() > 0, "API_TOKEN must not be empty");

fn main() -> Result<()> {
    // Link ESP-IDF patches and initialize logging
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // Set log level: Debug for dev builds, Info for release
    if cfg!(debug_assertions) {
        log::set_max_level(log::LevelFilter::Debug);
    } else {
        log::set_max_level(log::LevelFilter::Info);
    }

    // Quiet noisy ESP-IDF C components
    if let Err(e) = esp_idf_svc::log::set_target_level("wifi", log::LevelFilter::Warn) {
        warn!("Failed to set log level for 'wifi': {}", e);
    }
    if let Err(e) = esp_idf_svc::log::set_target_level("httpd", log::LevelFilter::Warn) {
        warn!("Failed to set log level for 'httpd': {}", e);
    }
    if let Err(e) = esp_idf_svc::log::set_target_level("httpd_ws", log::LevelFilter::Warn) {
        warn!("Failed to set log level for 'httpd_ws': {}", e);
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
    let (_server, engine, ws_senders) = start_http_server(nvs_dirty.clone(), API_TOKEN)?;
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

    // Main loop — read sensor, tick subscriptions, persist, keep Wi-Fi alive
    let mut wifi_retry_count: u32 = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if !wifi.is_connected()? {
            // Log on first attempt and every 12th (~once/min at 5s interval)
            if wifi_retry_count % 12 == 0 {
                warn!("Wi-Fi disconnected, reconnecting... attempt={}", wifi_retry_count + 1);
            }
            wifi_retry_count = wifi_retry_count.saturating_add(1);
            match connect_wifi(&mut wifi) {
                Ok(()) => {
                    let label = if wifi_retry_count == 1 { "attempt" } else { "attempts" };
                    info!("Wi-Fi reconnected after {} {}", wifi_retry_count, label);
                    wifi_retry_count = 0;
                }
                Err(e) => {
                    warn!("Wi-Fi reconnect failed: {}", e);
                    continue;
                }
            }
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

        // Tick subscriptions
        let deliveries = {
            let mut eng = lock_or_recover(&engine, "engine");
            eng.tick(now_secs())
        };
        dispatch_deliveries(&deliveries, &ws_senders, &engine);

        // Persist writable items to NVS if dirty
        if nvs_dirty.swap(false, Ordering::Acquire) {
            let eng = lock_or_recover(&engine, "engine");
            let items = collect_writable_items(&eng.tree);
            save_writable_items(&mut nvs_store, &items);
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
