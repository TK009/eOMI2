// HTTP/WebSocket server setup — ESP-only.
//
// Lock ordering: Engine before WsSenders. Never hold both simultaneously.
// The main loop and all handlers follow: lock(engine) → drop → lock(senders) → drop.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    build_read_op, is_successful_write_response, now_secs, omi_uri_to_odf_path,
    render_landing_page, uri_path, uri_query, BodyError, OmiReadParams,
};
use crate::omi::{Engine, OmiMessage, OmiResponse, Operation};
use crate::pages::{PageError, PageStore};

/// Monotonic counter for assigning unique WebSocket session IDs.
/// Avoids fd-reuse races where a new connection gets the same fd as a
/// recently closed one before the close handler fires.
static NEXT_WS_SESSION: AtomicU64 = AtomicU64::new(1);

pub type WsSenders = Arc<Mutex<BTreeMap<u64, EspHttpWsDetachedSender>>>;
/// Maps raw fd → monotonic session ID so the WS handler can look up the
/// session ID for an existing connection without allocating new IDs.
type FdToSession = Arc<Mutex<BTreeMap<i32, u64>>>;

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

pub fn start_http_server(
    nvs_dirty: Arc<AtomicBool>,
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
    let dirty = nvs_dirty.clone();
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

        let is_write = matches!(&msg.operation, Operation::Write(_));
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(msg, now_secs(), None)
        };
        if is_write && is_successful_write_response(&resp) {
            dirty.store(true, Ordering::Release);
        }
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
            eng.process(read_msg, now_secs(), None)
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
            eng.process(read_msg, now_secs(), None)
        };
        send_omi_json(req, &resp)?;
        Ok(())
    })?;

    // WS /omi/ws — WebSocket endpoint for persistent OMI connections.
    // Two locks used: Engine (for processing) and WsSenders (for send handles).
    // Lock ordering: always Engine before WsSenders; never hold both at once.
    let eng = engine.clone();
    let ws = ws_senders.clone();
    let fd_map = fd_to_session.clone();
    let dirty = nvs_dirty.clone();
    server.ws_handler("/omi/ws", move |conn| {
        if conn.is_new() {
            let sender = conn.create_detached_sender()?;
            let fd = conn.session();
            let session_id = NEXT_WS_SESSION.fetch_add(1, Ordering::Relaxed);
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
                let err = r#"{"omi":"1.0","ttl":0,"response":{"status":400,"desc":"Binary frames not supported"}}"#;
                conn.send(FrameType::Text(false), err.as_bytes())?;
                return Ok(());
            }
        }

        if len > MAX_WS_MSG {
            let err = r#"{"omi":"1.0","ttl":0,"response":{"status":413,"desc":"Message too large"}}"#;
            conn.send(FrameType::Text(false), err.as_bytes())?;
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
                let err = r#"{"omi":"1.0","ttl":0,"response":{"status":400,"desc":"Invalid UTF-8"}}"#;
                conn.send(FrameType::Text(false), err.as_bytes())?;
                return Ok(());
            }
        };
        let msg = match OmiMessage::parse(text) {
            Ok(m) => m,
            Err(e) => {
                let err = format!(
                    r#"{{"omi":"1.0","ttl":0,"response":{{"status":400,"desc":"Parse error: {}"}}}}"#,
                    e
                );
                conn.send(FrameType::Text(false), err.as_bytes())?;
                return Ok(());
            }
        };
        let is_write = matches!(&msg.operation, Operation::Write(_));
        let fd = conn.session();
        let session_id = fd_map.lock().unwrap_or_else(|e| e.into_inner())
            .get(&fd).copied().unwrap_or(0);
        let resp = {
            let mut eng = eng.lock().unwrap_or_else(|e| e.into_inner());
            eng.process(msg, now_secs(), Some(session_id))
        };
        if is_write && is_successful_write_response(&resp) {
            dirty.store(true, Ordering::Release);
        }
        match serde_json::to_string(&resp) {
            Ok(json) => conn.send(FrameType::Text(false), json.as_bytes())?,
            Err(e) => {
                warn!("WS response serialization failed: {}", e);
                let err = r#"{"omi":"1.0","ttl":0,"response":{"status":500,"desc":"Serialization error"}}"#;
                conn.send(FrameType::Text(false), err.as_bytes())?;
            }
        }
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

    Ok((server, engine, ws_senders))
}
