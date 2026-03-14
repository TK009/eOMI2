// HTTP/WebSocket server setup — ESP-only.
//
// Lock ordering: Engine before WsSenders. Never hold both simultaneously.
// The main loop and all handlers follow: lock(engine) → drop → lock(senders) → drop.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use std::sync::{Arc, Mutex};

use anyhow::Result;
use esp_idf_svc::{
    http::server::{Configuration as HttpConfig, EspHttpServer, ws::EspHttpWsDetachedSender},
    http::Method,
    io::{Read, Write},
    ws::FrameType,
};
use log::{debug, info, warn};

use crate::captive_portal::{
    self, ConnectionState, ConnectionStatus, ProvisionForm, ScannedNetwork,
};
use crate::compress;
use crate::http::{
    build_read_op, check_bearer_auth, is_mutating_operation, is_successful_write_response,
    now_secs, omi_uri_to_odf_path, render_landing_page, uri_path, uri_query,
    validate_content_length, BodyError, OmiReadParams,
};
use crate::log_util::RateLimiter;
use crate::omi::{Engine, OmiMessage, OmiResponse, Operation, SessionId};
use crate::omi::subscriptions::{Delivery, DeliveryTarget};
use crate::pages::{PageError, PageStore};
use crate::sync_util::lock_or_recover;
use crate::wifi_ap;

// ---------------------------------------------------------------------------
// Server mode & portal state
// ---------------------------------------------------------------------------

/// Server routing mode — controls which routes are active.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    /// Normal operation — OMI routes + page store.
    Normal = 0,
    /// Captive portal active — provisioning form + optional OMI routes.
    Portal = 1,
}

/// Pending provisioning request queued by POST /provision for the main loop.
pub struct PendingProvision {
    pub form: ProvisionForm,
    /// Plaintext API key if generated. Main loop hashes and saves, then drops.
    pub generated_api_key: Option<String>,
}

/// Configuration for rendering the captive portal form.
struct FormConfig {
    max_aps: usize,
    saved_ssids: Vec<String>,
    hostname: String,
    is_first_setup: bool,
    error_message: Option<String>,
}

/// Shared state for portal mode, accessed by HTTP handlers and main loop.
pub struct PortalState {
    mode: AtomicU8,
    pub connection_status: Mutex<ConnectionStatus>,
    pub pending_provision: Mutex<Option<PendingProvision>>,
    pub scan_results: Mutex<Vec<ScannedNetwork>>,
    form_config: Mutex<FormConfig>,
}

impl PortalState {
    pub fn new(
        mode: ServerMode,
        max_aps: usize,
        saved_ssids: Vec<String>,
        hostname: String,
        is_first_setup: bool,
    ) -> Self {
        Self {
            mode: AtomicU8::new(mode as u8),
            connection_status: Mutex::new(ConnectionStatus {
                state: ConnectionState::Idle,
                message: None,
                ip: None,
            }),
            pending_provision: Mutex::new(None),
            scan_results: Mutex::new(Vec::new()),
            form_config: Mutex::new(FormConfig {
                max_aps,
                saved_ssids,
                hostname,
                is_first_setup,
                error_message: None,
            }),
        }
    }

    /// Get the current server mode (lock-free).
    pub fn mode(&self) -> ServerMode {
        match self.mode.load(Ordering::Acquire) {
            1 => ServerMode::Portal,
            _ => ServerMode::Normal,
        }
    }

    /// Set the server mode (lock-free).
    pub fn set_mode(&self, mode: ServerMode) {
        self.mode.store(mode as u8, Ordering::Release);
    }

    /// Take the pending provision request for main loop processing.
    pub fn take_pending_provision(&self) -> Option<PendingProvision> {
        lock_or_recover(&self.pending_provision, "pending_provision").take()
    }

    /// Update the form error message (shown on next GET /).
    pub fn set_form_error(&self, msg: Option<String>) {
        lock_or_recover(&self.form_config, "form_config").error_message = msg;
    }

    /// Update form rendering config after provisioning.
    pub fn update_form_config(&self, saved_ssids: Vec<String>, hostname: String) {
        let mut cfg = lock_or_recover(&self.form_config, "form_config");
        cfg.saved_ssids = saved_ssids;
        cfg.hostname = hostname;
        cfg.is_first_setup = false;
    }
}

/// Check if OMI API routes should be denied (portal mode + deny flag).
fn is_api_denied(portal: &PortalState) -> bool {
    cfg!(feature = "deny_api_during_provisioning") && portal.mode() == ServerMode::Portal
}

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// Generate a random 32-character hex API key using ESP hardware RNG.
fn generate_api_key() -> String {
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(4) {
        let r = unsafe { esp_idf_svc::sys::esp_random() };
        let rb = r.to_ne_bytes();
        chunk.copy_from_slice(&rb[..chunk.len()]);
    }
    let mut hex = String::with_capacity(32);
    for b in bytes {
        hex.push(HEX_CHARS[(b >> 4) as usize] as char);
        hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    hex
}

// ---------------------------------------------------------------------------
// Existing helpers (unchanged)
// ---------------------------------------------------------------------------

/// Monotonic counter for assigning unique WebSocket session IDs.
static NEXT_WS_SESSION: AtomicU32 = AtomicU32::new(1);

pub type WsSenders = Arc<Mutex<BTreeMap<SessionId, EspHttpWsDetachedSender>>>;
pub type PendingDeliveries = Arc<Mutex<Vec<crate::omi::Delivery>>>;
type FdToSession = Arc<Mutex<BTreeMap<i32, SessionId>>>;

/// Read request body up to `max` bytes.
fn read_body(
    req: &mut esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    max: usize,
) -> std::result::Result<Vec<u8>, BodyError> {
    let content_len = validate_content_length(req.header("content-length"), max)?;
    let mut buf = vec![0u8; content_len];
    if let Err(e) = req.read_exact(&mut buf) {
        warn!("Body read failed: {}", e);
        return Err(BodyError::ReadFailed);
    }
    Ok(buf)
}

/// Send an HTTP response, logging any I/O failures instead of propagating them.
fn send_response(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    match req.into_response(status, Some(reason), headers) {
        Ok(mut resp) => {
            if let Err(e) = resp.write_all(body) {
                warn!("Response write failed: {}", e);
            }
        }
        Err(e) => warn!("Response start failed: {}", e),
    }
}

/// Send an HTML response, gzip-compressed if the client accepts it.
fn send_html(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    status: u16,
    reason: &str,
    html: &[u8],
) {
    let accept = req.header("accept-encoding").unwrap_or("");
    if accept.contains("gzip") {
        let compressed = compress::gzip_compress(html);
        let headers = [
            ("Content-Type", "text/html"),
            ("Content-Encoding", "gzip"),
        ];
        send_response(req, status, reason, &headers, &compressed);
    } else {
        let headers = [("Content-Type", "text/html")];
        send_response(req, status, reason, &headers, html);
    }
}

/// Map a `BodyError` to an HTTP error response.
fn send_body_error(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    err: BodyError,
    max_desc: &str,
) {
    match err {
        BodyError::Empty => send_response(req, 400, "Bad Request", &[], b"Empty body"),
        BodyError::TooLarge => send_response(req, 413, "Payload Too Large", &[], max_desc.as_bytes()),
        BodyError::Invalid => send_response(req, 400, "Bad Request", &[], b"Invalid Content-Length"),
        BodyError::ReadFailed => send_response(req, 500, "Internal Server Error", &[], b"Failed to read body"),
    }
}

fn send_omi_json(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    msg: &OmiMessage,
) {
    let headers = [("Content-Type", "application/json")];
    match serde_json::to_string(msg) {
        Ok(json) => {
            send_response(req, 200, "OK", &headers, json.as_bytes());
        }
        Err(e) => {
            warn!("OMI response serialization failed: {}", e);
            let err = OmiResponse::error("Serialization error");
            let json = serde_json::to_string(&err).unwrap_or_default();
            send_response(req, 500, "Internal Server Error", &headers, json.as_bytes());
        }
    }
}

fn send_ws_omi(
    conn: &mut esp_idf_svc::http::server::ws::EspHttpWsConnection,
    msg: OmiMessage,
) -> Result<()> {
    let json = serde_json::to_string(&msg).unwrap_or_else(|_|
        r#"{"omi":"1.0","ttl":0,"response":{"status":500,"desc":"Serialization error"}}"#.into()
    );
    conn.send(FrameType::Text(false), json.as_bytes())?;
    Ok(())
}

fn check_auth(
    req: &esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    token: &str,
) -> bool {
    check_bearer_auth(req.header("authorization"), token)
}

/// Dispatch subscription deliveries: send WebSocket frames, POST callbacks, skip poll.
pub fn dispatch_deliveries(
    deliveries: &[Delivery],
    ws_senders: &WsSenders,
    engine: &Arc<Mutex<Engine>>,
    rate_limiter: &mut RateLimiter,
) {
    if deliveries.is_empty() {
        return;
    }
    let mut failed_sessions: Vec<SessionId> = Vec::new();
    let mut pending_callbacks: Vec<(String, String, String)> = Vec::new();
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
                                warn!("WS delivery serialization failed session={}: {}", session, e);
                            }
                        }
                    }
                }
                DeliveryTarget::Callback(url) => {
                    let resp = OmiResponse::subscription_event(&d.rid, &d.path, &d.values);
                    match serde_json::to_string(&resp) {
                        Ok(json) => {
                            pending_callbacks.push((url.clone(), json, d.rid.clone()));
                        }
                        Err(e) => {
                            warn!("Callback delivery serialization failed: {}", e);
                        }
                    }
                }
                DeliveryTarget::Poll => {}
            }
        }
        for sid in &failed_sessions {
            senders.remove(sid);
        }
    }
    for (url, json, rid) in &pending_callbacks {
        crate::callback::deliver_callback(url, json.as_bytes(), rid);
    }
    if !failed_sessions.is_empty() {
        let mut eng = lock_or_recover(engine, "engine");
        for sid in &failed_sessions {
            eng.subscriptions().cancel_by_ws_session(*sid);
        }
    }
}

// ---------------------------------------------------------------------------
// Server setup
// ---------------------------------------------------------------------------

const HTTP_THREAD_STACK: usize = 16384;

pub fn start_http_server(
    nvs_dirty: Arc<AtomicBool>,
    api_token: &'static str,
    portal: Arc<PortalState>,
) -> Result<(EspHttpServer<'static>, Arc<Mutex<Engine>>, WsSenders, PendingDeliveries)> {
    let config = HttpConfig {
        http_port: 80,
        uri_match_wildcard: true,
        stack_size: HTTP_THREAD_STACK,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;

    let store = Arc::new(Mutex::new(PageStore::new()));
    let engine = Arc::new(Mutex::new(Engine::new()));
    let ws_senders: WsSenders = Arc::new(Mutex::new(BTreeMap::new()));
    let fd_to_session: FdToSession = Arc::new(Mutex::new(BTreeMap::new()));
    let pending_deliveries: PendingDeliveries = Arc::new(Mutex::new(Vec::new()));

    // Route registration order: exact before wildcard, WS before OMI wildcard,
    // OMI wildcard before page wildcard.

    // GET / — mode-aware: portal form vs landing page
    let s = store.clone();
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/", Method::Get, move |req| {
        if p.mode() == ServerMode::Portal {
            let cfg = lock_or_recover(&p.form_config, "form_config");
            let saved: Vec<&str> = cfg.saved_ssids.iter().map(|s| s.as_str()).collect();
            let html = captive_portal::render_provisioning_form(
                cfg.max_aps,
                &saved,
                &cfg.hostname,
                cfg.is_first_setup,
                cfg.error_message.as_deref(),
            );
            send_html(req, 200, "OK", html.as_bytes());
        } else {
            let store = lock_or_recover(&s, "page_store");
            let html = render_landing_page(&store);
            send_html(req, 200, "OK", html.as_bytes());
        }
        Ok(())
    })?;

    // POST / — accept HTML+JS payload (unchanged behavior)
    server.fn_handler::<Infallible, _>("/", Method::Post, |mut req| {
        const MAX_PAYLOAD: usize = 64 * 1024;
        let buf = match read_body(&mut req, MAX_PAYLOAD) {
            Ok(b) => b,
            Err(e) => { send_body_error(req, e, "Payload exceeds 64KB limit"); return Ok(()); }
        };

        let body = match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => {
                send_response(req, 400, "Bad Request", &[], b"Invalid UTF-8");
                return Ok(());
            }
        };
        info!("POST / len={}", body.len());
        debug!("Payload:\n{}", body);

        send_response(req, 200, "OK", &[], b"OK: payload received");
        Ok(())
    })?;

    // POST /provision — captive portal form submission (FR-016)
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/provision", Method::Post, move |mut req| {
        const MAX_FORM: usize = 4096;
        let buf = match read_body(&mut req, MAX_FORM) {
            Ok(b) => b,
            Err(e) => { send_body_error(req, e, "Form body exceeds 4KB limit"); return Ok(()); }
        };
        let body = match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => {
                send_response(req, 400, "Bad Request", &[], b"Invalid UTF-8");
                return Ok(());
            }
        };

        let (max_aps, is_first_setup) = {
            let cfg = lock_or_recover(&p.form_config, "form_config");
            (cfg.max_aps, cfg.is_first_setup)
        };
        match captive_portal::parse_provision_form(&body, max_aps, is_first_setup) {
            Ok(form) => {
                let ssid_count = form.credentials.len();
                let hostname = form.hostname.clone().unwrap_or_else(|| {
                    lock_or_recover(&p.form_config, "form_config").hostname.clone()
                });

                // Generate API key if requested
                let generated_key = if form.api_key_action == captive_portal::ApiKeyAction::Generate {
                    Some(generate_api_key())
                } else {
                    None
                };

                // Set connection status to Connecting
                *lock_or_recover(&p.connection_status, "connection_status") = ConnectionStatus {
                    state: ConnectionState::Connecting,
                    message: None,
                    ip: None,
                };

                // Queue for main loop processing
                *lock_or_recover(&p.pending_provision, "pending_provision") = Some(PendingProvision {
                    form,
                    generated_api_key: generated_key.clone(),
                });

                info!("Provision submitted: {} SSIDs, hostname={}", ssid_count, hostname);

                let html = captive_portal::render_provision_success(
                    generated_key.as_deref(),
                    &hostname,
                    ssid_count,
                );
                send_html(req, 200, "OK", html.as_bytes());
            }
            Err(e) => {
                let msg = match e {
                    captive_portal::FormError::NoCredentials => "At least one WiFi network is required",
                    captive_portal::FormError::EmptySsid => "SSID cannot be empty",
                    captive_portal::FormError::InvalidEncoding => "Invalid form encoding",
                    captive_portal::FormError::ApiKeyRequired => "API key is required on first setup",
                };
                let cfg = lock_or_recover(&p.form_config, "form_config");
                let saved: Vec<&str> = cfg.saved_ssids.iter().map(|s| s.as_str()).collect();
                let html = captive_portal::render_provisioning_form(
                    cfg.max_aps,
                    &saved,
                    &cfg.hostname,
                    cfg.is_first_setup,
                    Some(msg),
                );
                send_html(req, 400, "Bad Request", html.as_bytes());
            }
        }
        Ok(())
    })?;

    // GET /scan — WiFi scan results as JSON
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/scan", Method::Get, move |req| {
        let results = lock_or_recover(&p.scan_results, "scan_results");
        let headers = [("Content-Type", "application/json")];
        match serde_json::to_string(&*results) {
            Ok(json) => send_response(req, 200, "OK", &headers, json.as_bytes()),
            Err(_) => send_response(req, 200, "OK", &headers, b"[]"),
        }
        Ok(())
    })?;

    // GET /status — connection status as JSON (FR-016)
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/status", Method::Get, move |req| {
        let status = lock_or_recover(&p.connection_status, "connection_status");
        let headers = [("Content-Type", "application/json")];
        match serde_json::to_string(&*status) {
            Ok(json) => send_response(req, 200, "OK", &headers, json.as_bytes()),
            Err(_) => send_response(req, 200, "OK", &headers, b"{\"state\":\"idle\"}"),
        }
        Ok(())
    })?;

    // POST /omi — OMI message endpoint
    let eng = engine.clone();
    let dirty = nvs_dirty.clone();
    let pd = pending_deliveries.clone();
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/omi", Method::Post, move |mut req| {
        if is_api_denied(&p) {
            send_response(req, 503, "Service Unavailable", &[], b"API disabled during provisioning");
            return Ok(());
        }

        if let Some(ct) = req.header("content-type") {
            if !ct.contains("application/json") {
                send_response(req, 415, "Unsupported Media Type", &[], b"Expected application/json");
                return Ok(());
            }
        }

        const MAX_OMI: usize = 16 * 1024;
        let buf = match read_body(&mut req, MAX_OMI) {
            Ok(b) => b,
            Err(e) => { send_body_error(req, e, "Body exceeds 16KB limit"); return Ok(()); }
        };
        debug!("POST /omi len={}", buf.len());

        let text = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                send_response(req, 400, "Bad Request", &[], b"Invalid UTF-8");
                return Ok(());
            }
        };

        let msg = match OmiMessage::parse(text) {
            Ok(m) => m,
            Err(e) => {
                let err_msg = format!("Parse error: {}", e);
                send_response(req, 400, "Bad Request", &[], err_msg.as_bytes());
                return Ok(());
            }
        };

        if is_mutating_operation(&msg.operation) && !check_auth(&req, api_token) {
            let err = OmiResponse::unauthorized("Authentication required");
            send_omi_json(req, &err);
            return Ok(());
        }

        let is_write = matches!(&msg.operation, Operation::Write(_));
        let (resp, deliveries) = {
            let mut eng = lock_or_recover(&eng, "engine");
            eng.process(msg, now_secs(), None)
        };
        if is_write && is_successful_write_response(&resp) {
            dirty.store(true, Ordering::Release);
        }
        if !deliveries.is_empty() {
            lock_or_recover(&pd, "pending_deliveries").extend(deliveries);
        }
        send_omi_json(req, &resp);
        Ok(())
    })?;

    // GET /omi — REST root listing (exact match)
    let eng = engine.clone();
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/omi", Method::Get, move |req| {
        if is_api_denied(&p) {
            send_response(req, 503, "Service Unavailable", &[], b"API disabled during provisioning");
            return Ok(());
        }
        let params = uri_query(req.uri())
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op("/", &params);
        let (resp, _) = {
            let mut eng = lock_or_recover(&eng, "engine");
            eng.process(read_msg, now_secs(), None)
        };
        send_omi_json(req, &resp);
        Ok(())
    })?;

    // WS /omi/ws — WebSocket endpoint for persistent OMI connections.
    let eng = engine.clone();
    let ws = ws_senders.clone();
    let fd_map = fd_to_session.clone();
    let p = portal.clone();
    server.ws_handler("/omi/ws", move |conn| -> anyhow::Result<()> {
        if conn.is_new() {
            if is_api_denied(&p) {
                // Can't send HTTP error on WS upgrade, just close
                return Ok(());
            }
            let sender = conn.create_detached_sender()?;
            let fd = conn.session();
            let session_id = NEXT_WS_SESSION.fetch_add(1, Ordering::Relaxed);
            info!("WS connect: fd={}, session={}", fd, session_id);
            lock_or_recover(&fd_map, "fd_to_session").insert(fd, session_id);
            lock_or_recover(&ws, "ws_senders").insert(session_id, sender);
            return Ok(());
        }
        if conn.is_closed() {
            let fd = conn.session();
            let session_id = lock_or_recover(&fd_map, "fd_to_session").remove(&fd);
            if let Some(sid) = session_id {
                info!("WS close: fd={}, session={}", fd, sid);
                lock_or_recover(&eng, "engine")
                    .subscriptions()
                    .cancel_by_ws_session(sid);
                lock_or_recover(&ws, "ws_senders").remove(&sid);
            }
            return Ok(());
        }
        const MAX_WS_MSG: usize = 16 * 1024;
        let (frame_type, len) = conn.recv(&mut [])?;

        match frame_type {
            FrameType::Text(_) | FrameType::Continue(_) => {}
            FrameType::Ping => {
                conn.send(FrameType::Pong, &[])?;
                return Ok(());
            }
            FrameType::Pong | FrameType::Close | FrameType::SocketClose => {
                return Ok(());
            }
            FrameType::Binary(_) => {
                send_ws_omi(conn, OmiResponse::bad_request("Binary frames not supported"))?;
                return Ok(());
            }
        }

        if len > MAX_WS_MSG {
            send_ws_omi(conn, OmiResponse::payload_too_large("Message too large"))?;
            return Ok(());
        }
        let mut stack_buf = [0u8; 512];
        let mut heap_buf = Vec::new();
        let buf: &mut [u8] = if len <= stack_buf.len() {
            &mut stack_buf[..len]
        } else {
            heap_buf.resize(len, 0);
            &mut heap_buf
        };
        conn.recv(buf)?;
        let end = buf.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        let payload = &buf[..end];
        let text = match std::str::from_utf8(payload) {
            Ok(s) => s,
            Err(_) => {
                send_ws_omi(conn, OmiResponse::bad_request("Invalid UTF-8"))?;
                return Ok(());
            }
        };
        let msg = match OmiMessage::parse(text) {
            Ok(m) => m,
            Err(e) => {
                send_ws_omi(conn, OmiResponse::bad_request(&format!("Parse error: {}", e)))?;
                return Ok(());
            }
        };
        if matches!(&msg.operation, Operation::Write(_) | Operation::Delete(_)) {
            send_ws_omi(conn, OmiResponse::unauthorized(
                "Write/delete operations require the authenticated HTTP endpoint"
            ))?;
            return Ok(());
        }
        let fd = conn.session();
        let session_id = lock_or_recover(&fd_map, "fd_to_session")
            .get(&fd).copied().unwrap_or(0);
        let (resp, _) = {
            let mut eng = lock_or_recover(&eng, "engine");
            eng.process(msg, now_secs(), Some(session_id))
        };
        match serde_json::to_string(&resp) {
            Ok(json) => conn.send(FrameType::Text(false), json.as_bytes())?,
            Err(e) => {
                warn!("WS response serialization failed session={}: {}", session_id, e);
                send_ws_omi(conn, OmiResponse::error("Serialization error"))?;
            }
        }
        Ok(())
    })?;

    // GET /omi/* — REST discovery (wildcard)
    let eng = engine.clone();
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/omi/*", Method::Get, move |req| {
        if is_api_denied(&p) {
            send_response(req, 503, "Service Unavailable", &[], b"API disabled during provisioning");
            return Ok(());
        }
        let full_uri = req.uri();
        let path_part = uri_path(full_uri);
        let (odf_path, _trailing) = omi_uri_to_odf_path(path_part);
        let params = uri_query(full_uri)
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op(odf_path, &params);
        let (resp, _) = {
            let mut eng = lock_or_recover(&eng, "engine");
            eng.process(read_msg, now_secs(), None)
        };
        send_omi_json(req, &resp);
        Ok(())
    })?;

    // PATCH /* — store an HTML page at the given path
    let s = store.clone();
    server.fn_handler::<Infallible, _>("/*", Method::Patch, move |mut req| {
        if !check_auth(&req, api_token) {
            send_response(req, 401, "Unauthorized", &[], b"Authentication required");
            return Ok(());
        }
        let path = uri_path(req.uri()).to_string();

        const MAX_PAYLOAD: usize = 64 * 1024;
        let body = match read_body(&mut req, MAX_PAYLOAD) {
            Ok(b) => b,
            Err(e) => { send_body_error(req, e, "Payload exceeds 64KB limit"); return Ok(()); }
        };

        let html = match String::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                send_response(req, 400, "Bad Request", &[], b"Invalid UTF-8");
                return Ok(());
            }
        };
        let mut store = lock_or_recover(&s, "page_store");
        match store.store(&path, &html) {
            Ok(()) => {
                info!("PATCH path={} bytes={}", path, html.len());
                send_response(req, 200, "OK", &[], b"OK: page stored");
            }
            Err(PageError::ReservedPath) => {
                send_response(req, 403, "Forbidden", &[], b"Reserved path");
            }
            Err(PageError::InvalidPath) => {
                send_response(req, 400, "Bad Request", &[], b"Invalid path");
            }
            Err(PageError::PageTooLarge) => {
                send_response(req, 413, "Payload Too Large", &[], b"Page exceeds 64KB limit");
            }
            Err(PageError::StorageFull) => {
                send_response(req, 507, "Insufficient Storage", &[], b"Storage full");
            }
            Err(_) => {
                send_response(req, 500, "Internal Server Error", &[], b"Unexpected error");
            }
        }
        Ok(())
    })?;

    // DELETE /* — remove a stored page
    let s = store.clone();
    server.fn_handler::<Infallible, _>("/*", Method::Delete, move |req| {
        if !check_auth(&req, api_token) {
            send_response(req, 401, "Unauthorized", &[], b"Authentication required");
            return Ok(());
        }
        let path = uri_path(req.uri()).to_string();
        let mut store = lock_or_recover(&s, "page_store");
        match store.remove(&path) {
            Ok(()) => {
                info!("DELETE {} — removed", path);
                send_response(req, 200, "OK", &[], b"OK: page removed");
            }
            Err(PageError::NotFound) => {
                send_response(req, 404, "Not Found", &[], b"Page not found");
            }
            Err(_) => {
                send_response(req, 500, "Internal Server Error", &[], b"Unexpected error");
            }
        }
        Ok(())
    })?;

    // GET /* — mode-aware: portal redirect vs page serve
    let s = store.clone();
    let p = portal.clone();
    server.fn_handler::<Infallible, _>("/*", Method::Get, move |req| {
        let path = uri_path(req.uri()).to_string();

        // In portal mode, redirect non-portal/non-OMI paths to captive portal (FR-014)
        if p.mode() == ServerMode::Portal && captive_portal::should_redirect_to_portal("GET", &path) {
            let (status, headers, body) = captive_portal::redirect_to_form(wifi_ap::AP_IP);
            let h = [(headers[0].0, headers[0].1.as_str())];
            send_response(req, status, "Found", &h, body);
            return Ok(());
        }

        let store = lock_or_recover(&s, "page_store");
        match store.get(&path) {
            Some(html) => {
                send_html(req, 200, "OK", html.as_bytes());
            }
            None => {
                send_response(req, 404, "Not Found", &[], b"Page not found");
            }
        }
        Ok(())
    })?;

    Ok((server, engine, ws_senders, pending_deliveries))
}
