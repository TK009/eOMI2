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
use reconfigurable_device::http::{
    render_landing_page, uri_query, omi_uri_to_odf_path, OmiReadParams,
};
use reconfigurable_device::omi::{Engine, OmiMessage, OmiResponse, Operation, ReadOp};
use reconfigurable_device::pages::{PageError, PageStore};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

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
    let (_server, engine) = start_http_server()?;
    info!("HTTP server listening on port 80");

    // Main loop — tick subscriptions and keep Wi-Fi alive
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if !wifi.is_connected()? {
            warn!("Wi-Fi disconnected, reconnecting...");
            connect_wifi(&mut wifi)?;
        }
        let deliveries = {
            let mut eng = engine.lock().unwrap_or_else(|e| e.into_inner());
            eng.tick(now_secs())
        };
        for d in &deliveries {
            info!(
                "Sub delivery: rid={}, path={}, {} values (callback not yet implemented)",
                d.rid, d.path, d.values.len()
            );
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

/// Strip query string from URI, returning just the path.
fn uri_path(uri: &str) -> &str {
    uri.split('?').next().unwrap_or(uri)
}

enum BodyError {
    Empty,
    TooLarge,
}

/// Read request body up to `max` bytes.
fn read_body(
    req: &mut esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    max: usize,
) -> Result<std::result::Result<Vec<u8>, BodyError>, anyhow::Error> {
    let content_len = req
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_len == 0 {
        return Ok(Err(BodyError::Empty));
    }
    if content_len > max {
        return Ok(Err(BodyError::TooLarge));
    }
    let mut buf = vec![0u8; content_len];
    req.read_exact(&mut buf)?;
    Ok(Ok(buf))
}

/// Build a ReadOp from an O-DF path and parsed query parameters.
fn build_read_op(odf_path: &str, params: &OmiReadParams) -> OmiMessage {
    OmiMessage {
        version: "1.0".into(),
        ttl: 0,
        operation: Operation::Read(ReadOp {
            path: Some(odf_path.into()),
            rid: None,
            newest: params.newest,
            oldest: params.oldest,
            begin: params.begin,
            end: params.end,
            depth: params.depth,
            interval: None,
            callback: None,
        }),
    }
}

/// Serialize an OmiMessage response and write it as JSON to the HTTP response.
fn send_omi_json(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    msg: &OmiMessage,
) -> Result<()> {
    let json = serde_json::to_string(msg)?;
    let headers = [("Content-Type", "application/json")];
    req.into_response(200, Some("OK"), &headers)?
        .write_all(json.as_bytes())?;
    Ok(())
}

fn start_http_server() -> Result<(EspHttpServer<'static>, Arc<Mutex<Engine>>)> {
    let config = HttpConfig {
        http_port: 80,
        uri_match_wildcard: true,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;

    let store = Arc::new(Mutex::new(PageStore::new()));
    let engine = Arc::new(Mutex::new(Engine::new()));

    // Route registration order: exact before wildcard, OMI wildcard before page wildcard.

    // GET / — landing page with list of stored pages
    let s = store.clone();
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, move |req| {
        let store = s.lock().unwrap_or_else(|e| e.into_inner());
        let html = render_landing_page(&store);
        req.into_ok_response()?
            .write_all(html.as_bytes())?;
        Ok(())
    })?;

    // POST / — accept HTML+JS payload (unchanged behavior)
    server.fn_handler::<anyhow::Error, _>("/", Method::Post, |mut req| {
        // Cap at 64KB to prevent OOM on constrained devices
        const MAX_PAYLOAD: usize = 64 * 1024;
        let buf = match read_body(&mut req, MAX_PAYLOAD)? {
            Ok(b) => b,
            Err(BodyError::Empty) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Empty payload")?;
                return Ok(());
            }
            Err(BodyError::TooLarge) => {
                req.into_response(413, Some("Payload Too Large"), &[])?
                    .write_all(b"Payload exceeds 64KB limit")?;
                return Ok(());
            }
        };

        let body = match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Invalid UTF-8")?;
                return Ok(());
            }
        };
        info!("POST / received {} bytes", body.len());
        info!("Payload:\n{}", body);

        // TODO: parse HTML, extract <script> tags, execute JS
        req.into_ok_response()?
            .write_all(b"OK: payload received")?;
        Ok(())
    })?;

    // POST /omi — OMI message endpoint
    let eng = engine.clone();
    server.fn_handler::<anyhow::Error, _>("/omi", Method::Post, move |mut req| {
        // Content-Type check: reject non-JSON (allow missing/empty)
        if let Some(ct) = req.header("content-type") {
            if !ct.contains("application/json") {
                req.into_response(415, Some("Unsupported Media Type"), &[])?
                    .write_all(b"Expected application/json")?;
                return Ok(());
            }
        }

        const MAX_OMI: usize = 16 * 1024;
        let buf = match read_body(&mut req, MAX_OMI)? {
            Ok(b) => b,
            Err(BodyError::Empty) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Empty body")?;
                return Ok(());
            }
            Err(BodyError::TooLarge) => {
                req.into_response(413, Some("Payload Too Large"), &[])?
                    .write_all(b"Body exceeds 16KB limit")?;
                return Ok(());
            }
        };

        let text = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Invalid UTF-8")?;
                return Ok(());
            }
        };

        let msg = match OmiMessage::parse(text) {
            Ok(m) => m,
            Err(e) => {
                let err_msg = format!("Parse error: {}", e);
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(err_msg.as_bytes())?;
                return Ok(());
            }
        };

        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(msg, now_secs())
        };
        send_omi_json(req, &resp)?;
        Ok(())
    })?;

    // GET /omi — REST root listing (exact match)
    let eng = engine.clone();
    server.fn_handler::<anyhow::Error, _>("/omi", Method::Get, move |req| {
        let params = uri_query(req.uri())
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op("/", &params);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(read_msg, now_secs())
        };
        send_omi_json(req, &resp)?;
        Ok(())
    })?;

    // GET /omi/* — REST discovery (wildcard)
    let eng = engine.clone();
    server.fn_handler::<anyhow::Error, _>("/omi/*", Method::Get, move |req| {
        let full_uri = req.uri();
        let path_part = uri_path(full_uri);
        let (odf_path, _trailing) = omi_uri_to_odf_path(path_part);
        let params = uri_query(full_uri)
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op(odf_path, &params);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(read_msg, now_secs())
        };
        send_omi_json(req, &resp)?;
        Ok(())
    })?;

    // PATCH /* — store an HTML page at the given path
    let s = store.clone();
    server.fn_handler::<anyhow::Error, _>("/*", Method::Patch, move |mut req| {
        let path = uri_path(req.uri()).to_string();

        const MAX_PAYLOAD: usize = 64 * 1024;
        let body = match read_body(&mut req, MAX_PAYLOAD)? {
            Ok(b) => b,
            Err(BodyError::Empty) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Empty payload")?;
                return Ok(());
            }
            Err(BodyError::TooLarge) => {
                req.into_response(413, Some("Payload Too Large"), &[])?
                    .write_all(b"Payload exceeds 64KB limit")?;
                return Ok(());
            }
        };

        let html = match String::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Invalid UTF-8")?;
                return Ok(());
            }
        };
        let mut store = s.lock().unwrap_or_else(|e| e.into_inner());
        match store.store(&path, &html) {
            Ok(()) => {
                info!("PATCH {} — stored {} bytes", path, html.len());
                req.into_ok_response()?
                    .write_all(b"OK: page stored")?;
            }
            Err(PageError::ReservedPath) => {
                req.into_response(403, Some("Forbidden"), &[])?
                    .write_all(b"Reserved path")?;
            }
            Err(PageError::InvalidPath) => {
                req.into_response(400, Some("Bad Request"), &[])?
                    .write_all(b"Invalid path")?;
            }
            Err(PageError::PageTooLarge) => {
                req.into_response(413, Some("Payload Too Large"), &[])?
                    .write_all(b"Page exceeds 64KB limit")?;
            }
            Err(PageError::StorageFull) => {
                req.into_response(507, Some("Insufficient Storage"), &[])?
                    .write_all(b"Storage full")?;
            }
            Err(_) => {
                req.into_response(500, Some("Internal Server Error"), &[])?
                    .write_all(b"Unexpected error")?;
            }
        }
        Ok(())
    })?;

    // DELETE /* — remove a stored page
    let s = store.clone();
    server.fn_handler::<anyhow::Error, _>("/*", Method::Delete, move |req| {
        let path = uri_path(req.uri()).to_string();
        let mut store = s.lock().unwrap_or_else(|e| e.into_inner());
        match store.remove(&path) {
            Ok(()) => {
                info!("DELETE {} — removed", path);
                req.into_ok_response()?
                    .write_all(b"OK: page removed")?;
            }
            Err(PageError::NotFound) => {
                req.into_response(404, Some("Not Found"), &[])?
                    .write_all(b"Page not found")?;
            }
            Err(_) => {
                req.into_response(500, Some("Internal Server Error"), &[])?
                    .write_all(b"Unexpected error")?;
            }
        }
        Ok(())
    })?;

    // GET /* — serve a stored page
    let s = store.clone();
    server.fn_handler::<anyhow::Error, _>("/*", Method::Get, move |req| {
        let path = uri_path(req.uri()).to_string();
        let store = s.lock().unwrap_or_else(|e| e.into_inner());
        match store.get(&path) {
            Some(html) => {
                let headers = [("Content-Type", "text/html")];
                req.into_response(200, Some("OK"), &headers)?
                    .write_all(html.as_bytes())?;
            }
            None => {
                req.into_response(404, Some("Not Found"), &[])?
                    .write_all(b"Page not found")?;
            }
        }
        Ok(())
    })?;

    Ok((server, engine))
}
