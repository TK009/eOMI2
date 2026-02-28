use anyhow::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::Peripherals,
    http::server::{Configuration as HttpConfig, EspHttpServer},
    http::Method,
    io::{Read, Write},
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::{info, warn};

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");

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

    // Connect to Wi-Fi
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;
    connect_wifi(&mut wifi)?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected. IP: {}", ip_info.ip);

    // Start HTTP server
    let _server = start_http_server()?;
    info!("HTTP server listening on port 80");

    // Keep main thread alive — the HTTP server runs in ESP-IDF's own threads
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if !wifi.is_connected()? {
            warn!("Wi-Fi disconnected, reconnecting...");
            connect_wifi(&mut wifi)?;
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

fn start_http_server() -> Result<EspHttpServer<'static>> {
    let config = HttpConfig {
        http_port: 80,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;

    // GET / — simple status page
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        let html = "<!DOCTYPE html>\
            <html><body>\
            <h1>Reconfigurable Device</h1>\
            <p>Status: running</p>\
            <p>POST HTML+JS to <code>/</code> to program this device.</p>\
            </body></html>";
        req.into_ok_response()?
            .write_all(html.as_bytes())?;
        Ok(())
    })?;

    // POST / — accept HTML+JS payload
    server.fn_handler::<anyhow::Error, _>("/", Method::Post, |mut req| {
        let content_len = req
            .header("content-length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if content_len == 0 {
            req.into_response(400, Some("Bad Request"), &[])?
                .write_all(b"Empty payload")?;
            return Ok(());
        }

        // Cap at 64KB to prevent OOM on constrained devices
        const MAX_PAYLOAD: usize = 64 * 1024;
        if content_len > MAX_PAYLOAD {
            req.into_response(413, Some("Payload Too Large"), &[])?
                .write_all(b"Payload exceeds 64KB limit")?;
            return Ok(());
        }

        let mut buf = vec![0u8; content_len];
        req.read_exact(&mut buf)?;

        let body = String::from_utf8_lossy(&buf);
        info!("POST / received {} bytes", content_len);
        info!("Payload:\n{}", body);

        // TODO: parse HTML, extract <script> tags, execute JS
        req.into_ok_response()?
            .write_all(b"OK: payload received")?;
        Ok(())
    })?;

    Ok(server)
}
