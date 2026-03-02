// HTTP/WebSocket server setup — ESP-only.
//
// Lock ordering: Engine before WsSenders. Never hold both simultaneously.
// The main loop and all handlers follow: lock(engine) → drop → lock(senders) → drop.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use esp_idf_svc::{
    http::server::{Configuration as HttpConfig, EspHttpServer, ws::EspHttpWsDetachedSender},
    http::Method,
    io::{Read, Write},
    ws::FrameType,
};
use log::{info, warn};

use crate::http::{
    build_read_op, is_mutating_operation, is_successful_write_response, now_secs,
    omi_uri_to_odf_path, render_landing_page, uri_path, uri_query, BodyError, OmiReadParams,
};
use crate::omi::{Engine, OmiMessage, OmiResponse, Operation};
use crate::pages::{PageError, PageStore};

/// Monotonic counter for assigning unique WebSocket session IDs.
/// Avoids fd-reuse races where a new connection gets the same fd as a
/// recently closed one before the close handler fires.
static NEXT_WS_SESSION: AtomicU32 = AtomicU32::new(1);

pub type WsSenders = Arc<Mutex<BTreeMap<u64, EspHttpWsDetachedSender>>>;
/// Maps raw fd → monotonic session ID so the WS handler can look up the
/// session ID for an existing connection without allocating new IDs.
///
/// Note: WS upgrade requests cannot be authenticated because
/// `EspHttpWsConnection` does not expose HTTP headers.  Write and delete
/// operations are rejected at the message level; other state-modifying HTTP
/// endpoints (PATCH, DELETE) require Bearer auth.
type FdToSession = Arc<Mutex<BTreeMap<i32, u64>>>;

/// Read request body up to `max` bytes.
fn read_body(
    req: &mut esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    max: usize,
) -> std::result::Result<Vec<u8>, BodyError> {
    let content_len = req
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_len == 0 {
        return Err(BodyError::Empty);
    }
    if content_len > max {
        return Err(BodyError::TooLarge);
    }
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

/// Map a `BodyError` to an HTTP error response.
fn send_body_error(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    err: BodyError,
    max_desc: &str,
) {
    match err {
        BodyError::Empty => send_response(req, 400, "Bad Request", &[], b"Empty body"),
        BodyError::TooLarge => send_response(req, 413, "Payload Too Large", &[], max_desc.as_bytes()),
        BodyError::ReadFailed => send_response(req, 500, "Internal Server Error", &[], b"Failed to read body"),
    }
}

/// Serialize an OmiMessage response and write it as JSON to the HTTP response.
/// On serialization failure, sends a 500 with a structured OMI error.
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

/// Serialize an OmiMessage and send as a WS text frame.
/// Falls back to a hand-written JSON string if serialization itself fails.
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

/// Constant-time byte comparison to prevent timing side-channel attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Check if the request carries a valid `Authorization: Bearer <token>` header.
fn check_auth(
    req: &esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection>,
    token: &str,
) -> bool {
    req.header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
        .unwrap_or(false)
}

pub fn start_http_server(
    nvs_dirty: Arc<AtomicBool>,
    api_token: &'static str,
) -> Result<(EspHttpServer<'static>, Arc<Mutex<Engine>>, WsSenders)> {
    let config = HttpConfig {
        http_port: 80,
        uri_match_wildcard: true,
        ..Default::default()
    };
    let mut server = EspHttpServer::new(&config)?;

    let store = Arc::new(Mutex::new(PageStore::new()));
    let engine = Arc::new(Mutex::new(Engine::new()));
    let ws_senders: WsSenders = Arc::new(Mutex::new(BTreeMap::new()));
    let fd_to_session: FdToSession = Arc::new(Mutex::new(BTreeMap::new()));

    // Route registration order: exact before wildcard, WS before OMI wildcard,
    // OMI wildcard before page wildcard.

    // GET / — landing page with list of stored pages
    let s = store.clone();
    server.fn_handler::<Infallible, _>("/", Method::Get, move |req| {
        let store = s.lock().unwrap_or_else(|e| e.into_inner());
        let html = render_landing_page(&store);
        let headers = [("Content-Type", "text/html")];
        send_response(req, 200, "OK", &headers, html.as_bytes());
        Ok(())
    })?;

    // POST / — accept HTML+JS payload (unchanged behavior)
    server.fn_handler::<Infallible, _>("/", Method::Post, |mut req| {
        // Cap at 64KB to prevent OOM on constrained devices
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
        info!("POST / received {} bytes", body.len());
        info!("Payload:\n{}", body);

        // TODO: parse HTML, extract <script> tags, execute JS
        send_response(req, 200, "OK", &[], b"OK: payload received");
        Ok(())
    })?;

    // POST /omi — OMI message endpoint
    let eng = engine.clone();
    let dirty = nvs_dirty.clone();
    server.fn_handler::<Infallible, _>("/omi", Method::Post, move |mut req| {
        // Content-Type check: reject non-JSON (allow missing/empty)
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
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(msg, now_secs(), None)
        };
        if is_write && is_successful_write_response(&resp) {
            dirty.store(true, Ordering::Release);
        }
        send_omi_json(req, &resp);
        Ok(())
    })?;

    // GET /omi — REST root listing (exact match)
    let eng = engine.clone();
    server.fn_handler::<Infallible, _>("/omi", Method::Get, move |req| {
        let params = uri_query(req.uri())
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op("/", &params);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(read_msg, now_secs(), None)
        };
        send_omi_json(req, &resp);
        Ok(())
    })?;

    // WS /omi/ws — WebSocket endpoint for persistent OMI connections.
    // Must be registered BEFORE the GET /omi/* wildcard, otherwise the
    // wildcard's GET handler claims the /omi/ws path first and the WS
    // registration fails with ESP_ERR_HTTPD_HANDLER_EXISTS.
    // Two locks used: Engine (for processing) and WsSenders (for send handles).
    // Lock ordering: always Engine before WsSenders; never hold both at once.
    let eng = engine.clone();
    let ws = ws_senders.clone();
    let fd_map = fd_to_session.clone();
    server.ws_handler("/omi/ws", move |conn| -> anyhow::Result<()> {
        if conn.is_new() {
            let sender = conn.create_detached_sender()?;
            let fd = conn.session();
            let session_id = NEXT_WS_SESSION.fetch_add(1, Ordering::Relaxed) as u64;
            info!("WS connect: fd={}, session={}", fd, session_id);
            fd_map.lock().unwrap_or_else(|e| e.into_inner()).insert(fd, session_id);
            ws.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id, sender);
            return Ok(());
        }
        if conn.is_closed() {
            let fd = conn.session();
            let session_id = fd_map.lock().unwrap_or_else(|e| e.into_inner()).remove(&fd);
            if let Some(sid) = session_id {
                info!("WS close: fd={}, session={}", fd, sid);
                // Lock Engine before WsSenders (documented ordering invariant)
                eng.lock().unwrap_or_else(|e| e.into_inner())
                    .subscriptions()
                    .cancel_by_ws_session(sid);
                ws.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            }
            return Ok(());
        }
        // Receive frame — first call with empty buf to get length and type
        const MAX_WS_MSG: usize = 16 * 1024;
        let (frame_type, len) = conn.recv(&mut [])?;

        // Handle control and non-text frames
        match frame_type {
            FrameType::Text(_) | FrameType::Continue(_) => {} // process below
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
        // Use stack buffer for small messages to avoid heap allocation
        let mut stack_buf = [0u8; 512];
        let mut heap_buf = Vec::new();
        let buf: &mut [u8] = if len <= stack_buf.len() {
            &mut stack_buf[..len]
        } else {
            heap_buf.resize(len, 0);
            &mut heap_buf
        };
        conn.recv(buf)?;
        let text = match std::str::from_utf8(buf) {
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
        // Reject state-modifying operations over WS — clients must use the
        // authenticated HTTP endpoints for writes and deletes.
        if matches!(&msg.operation, Operation::Write(_) | Operation::Delete(_)) {
            send_ws_omi(conn, OmiResponse::unauthorized(
                "Write/delete operations require the authenticated HTTP endpoint"
            ))?;
            return Ok(());
        }
        let fd = conn.session();
        let session_id = fd_map.lock().unwrap_or_else(|e| e.into_inner())
            .get(&fd).copied().unwrap_or(0);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(msg, now_secs(), Some(session_id))
        };
        match serde_json::to_string(&resp) {
            Ok(json) => conn.send(FrameType::Text(false), json.as_bytes())?,
            Err(e) => {
                warn!("WS response serialization failed: {}", e);
                send_ws_omi(conn, OmiResponse::error("Serialization error"))?;
            }
        }
        Ok(())
    })?;

    // GET /omi/* — REST discovery (wildcard)
    let eng = engine.clone();
    server.fn_handler::<Infallible, _>("/omi/*", Method::Get, move |req| {
        let full_uri = req.uri();
        let path_part = uri_path(full_uri);
        let (odf_path, _trailing) = omi_uri_to_odf_path(path_part);
        let params = uri_query(full_uri)
            .map(OmiReadParams::from_query)
            .unwrap_or_default();
        let read_msg = build_read_op(odf_path, &params);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut store = s.lock().unwrap_or_else(|e| e.into_inner());
        match store.store(&path, &html) {
            Ok(()) => {
                info!("PATCH {} — stored {} bytes", path, html.len());
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
        let mut store = s.lock().unwrap_or_else(|e| e.into_inner());
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

    // GET /* — serve a stored page
    let s = store.clone();
    server.fn_handler::<Infallible, _>("/*", Method::Get, move |req| {
        let path = uri_path(req.uri()).to_string();
        let store = s.lock().unwrap_or_else(|e| e.into_inner());
        match store.get(&path) {
            Some(html) => {
                let headers = [("Content-Type", "text/html")];
                send_response(req, 200, "OK", &headers, html.as_bytes());
            }
            None => {
                send_response(req, 404, "Not Found", &[], b"Page not found");
            }
        }
        Ok(())
    })?;

    Ok((server, engine, ws_senders))
}
