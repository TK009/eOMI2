//! WiFi bridge firmware for E2E provisioning tests.
//!
//! Connects to a DUT's soft-AP and forwards HTTP requests from the host
//! over serial UART. Protocol: JSON lines in, `!`-prefixed JSON lines out.

use std::io::{BufRead, BufReader, Write as _};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::io::{Read as EmbRead, Write as _};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use log::*;

// Max HTTP response body we'll buffer (64 KB — portal pages are small).
const MAX_BODY: usize = 64 * 1024;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // Turn off the onboard WS2812 RGB LED (GPIO 18 on Saola-1 boards).
    // Set output LOW *before* enabling output direction to avoid the glitch
    // where PinDriver::output() enables the driver first, giving the WS2812
    // time to latch a white pixel.
    unsafe {
        use esp_idf_svc::sys::{gpio_reset_pin, gpio_set_level, gpio_set_direction, gpio_mode_t_GPIO_MODE_OUTPUT};
        gpio_reset_pin(18);
        gpio_set_level(18, 0);
        gpio_set_direction(18, gpio_mode_t_GPIO_MODE_OUTPUT);
    }

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs)).unwrap(),
        sysloop,
    )
    .unwrap();

    // Start WiFi in idle STA mode (not connected to anything yet).
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .unwrap();
    wifi.start().unwrap();

    respond_ok("ready", &[]);

    // Main command loop — read JSON lines from stdin.
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                error!("stdin read error: {}", e);
                continue;
            }
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        dispatch(&line, &mut wifi);
    }
}

fn dispatch(line: &str, wifi: &mut BlockingWifi<EspWifi<'static>>) {
    let cmd = json_str(line, "cmd");
    match cmd.as_deref() {
        Some("ping") => respond_ok("ping", &[]),
        Some("scan") => cmd_scan(wifi),
        Some("connect") => cmd_connect(line, wifi),
        Some("disconnect") => cmd_disconnect(wifi),
        Some("status") => cmd_status(wifi),
        Some("http") => cmd_http(line),
        _ => respond_err("unknown command"),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_scan(wifi: &mut BlockingWifi<EspWifi<'static>>) {
    match wifi.scan() {
        Ok(aps) => {
            let mut nets = String::from("[");
            for (i, ap) in aps.iter().enumerate() {
                if i > 0 {
                    nets.push(',');
                }
                let auth = match ap.auth_method.unwrap_or(AuthMethod::None) {
                    AuthMethod::None => "open",
                    AuthMethod::WEP => "wep",
                    AuthMethod::WPA => "wpa",
                    AuthMethod::WPA2Personal => "wpa2",
                    AuthMethod::WPA3Personal => "wpa3",
                    _ => "other",
                };
                nets.push_str(&format!(
                    "{{\"ssid\":\"{}\",\"rssi\":{},\"auth\":\"{}\"}}",
                    json_escape(&ap.ssid),
                    ap.signal_strength,
                    auth,
                ));
            }
            nets.push(']');
            print_response(&format!(
                "!{{\"ok\":true,\"type\":\"scan\",\"networks\":{}}}",
                nets
            ));
        }
        Err(e) => respond_err(&format!("scan failed: {}", e)),
    }
}

fn cmd_connect(line: &str, wifi: &mut BlockingWifi<EspWifi<'static>>) {
    let ssid = match json_str(line, "ssid") {
        Some(s) => s,
        None => return respond_err("missing ssid"),
    };
    let pass = json_str(line, "pass").unwrap_or_default();

    let auth = if pass.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };

    let conf = ClientConfiguration {
        ssid: ssid.as_str().try_into().unwrap_or_default(),
        password: pass.as_str().try_into().unwrap_or_default(),
        auth_method: auth,
        ..Default::default()
    };

    if let Err(e) = wifi.set_configuration(&Configuration::Client(conf)) {
        return respond_err(&format!("set config failed: {}", e));
    }

    if let Err(e) = wifi.connect() {
        return respond_err(&format!("connect failed: {}", e));
    }
    if let Err(e) = wifi.wait_netif_up() {
        return respond_err(&format!("netif up failed: {}", e));
    }

    let ip = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .map(|info| format!("{}", info.ip))
        .unwrap_or_default();

    print_response(&format!(
        "!{{\"ok\":true,\"type\":\"connect\",\"ip\":\"{}\"}}",
        ip
    ));
}

fn cmd_disconnect(wifi: &mut BlockingWifi<EspWifi<'static>>) {
    let _ = wifi.disconnect();
    respond_ok("disconnect", &[]);
}

fn cmd_status(wifi: &mut BlockingWifi<EspWifi<'static>>) {
    let connected = wifi.is_connected().unwrap_or(false);
    let ip = if connected {
        wifi.wifi()
            .sta_netif()
            .get_ip_info()
            .map(|info| format!("{}", info.ip))
            .unwrap_or_default()
    } else {
        String::new()
    };
    print_response(&format!(
        "!{{\"ok\":true,\"type\":\"status\",\"connected\":{},\"ip\":\"{}\"}}",
        connected, ip
    ));
}

fn cmd_http(line: &str) {
    let method = json_str(line, "method").unwrap_or_else(|| "GET".into());
    let url = match json_str(line, "url") {
        Some(u) => u,
        None => return respond_err("missing url"),
    };
    let body = json_str(line, "body").unwrap_or_default();
    let content_type = json_str(line, "content_type").unwrap_or_default();
    let authorization = json_str(line, "authorization").unwrap_or_default();

    let config = HttpConfig {
        buffer_size: Some(2048),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(15)),
        ..Default::default()
    };

    let conn = match EspHttpConnection::new(&config) {
        Ok(c) => c,
        Err(e) => return respond_err(&format!("http connection error: {}", e)),
    };

    let mut client = HttpClient::wrap(conn);

    // Build headers list from optional fields.
    let mut hdrs: Vec<(&str, &str)> = Vec::new();
    if !content_type.is_empty() {
        hdrs.push(("Content-Type", &content_type));
    }
    if !authorization.is_empty() {
        hdrs.push(("Authorization", &authorization));
    }

    let result = match method.as_str() {
        "GET" => client.request(embedded_svc::http::Method::Get, &url, &hdrs)
            .and_then(|r| r.submit()),
        "POST" => {
            client
                .post(&url, &hdrs)
                .and_then(|mut r| {
                    if !body.is_empty() {
                        r.write_all(body.as_bytes())?;
                        r.flush()?;
                    }
                    r.submit()
                })
        }
        _ => return respond_err(&format!("unsupported method: {}", method)),
    };

    match result {
        Ok(mut resp) => {
            let status = resp.status();

            // Collect headers we care about.
            let ct = resp
                .header("content-type")
                .unwrap_or("")
                .to_string();
            let location = resp
                .header("location")
                .unwrap_or("")
                .to_string();

            // Read body via embedded_svc::io::Read.
            let mut buf = vec![0u8; MAX_BODY];
            let mut total = 0;
            loop {
                match EmbRead::read(&mut resp, &mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => {
                        total += n;
                        if total >= MAX_BODY {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let body_str = String::from_utf8_lossy(&buf[..total]);

            let mut headers_json = String::from("{");
            let mut first = true;
            if !ct.is_empty() {
                headers_json.push_str(&format!("\"content-type\":\"{}\"", json_escape(&ct)));
                first = false;
            }
            if !location.is_empty() {
                if !first {
                    headers_json.push(',');
                }
                headers_json.push_str(&format!(
                    "\"location\":\"{}\"",
                    json_escape(&location)
                ));
            }
            headers_json.push('}');

            print_response(&format!(
                "!{{\"ok\":true,\"type\":\"http\",\"status\":{},\"body\":\"{}\",\"headers\":{}}}",
                status,
                json_escape(&body_str),
                headers_json,
            ));
        }
        Err(e) => respond_err(&format!("http request failed: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// JSON helpers (hand-rolled — no serde, vocabulary is fixed and tiny)
// ---------------------------------------------------------------------------

/// Extract a string value for a given key from a JSON object string.
/// Only matches keys at object boundaries (after `{` or `,`), not inside values.
fn json_str(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let mut search_from = 0;
    loop {
        let idx = json[search_from..].find(&pattern)?;
        let abs_idx = search_from + idx;
        // Verify this is a key position: preceded by `{` or `,` (ignoring whitespace)
        let before = json[..abs_idx].trim_end();
        if before.ends_with('{') || before.ends_with(',') {
            let rest = &json[abs_idx + pattern.len()..];
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix(':') {
                let rest = rest.trim_start();
                return parse_json_string_value(rest);
            }
        }
        search_from = abs_idx + 1;
    }
}

fn parse_json_string_value(rest: &str) -> Option<String> {
    if rest.starts_with('"') {
        // String value — parse until unescaped quote
        let rest = &rest[1..];
        let mut result = String::new();
        let mut chars = rest.chars();
        loop {
            match chars.next() {
                None => break,
                Some('"') => break,
                Some('\\') => {
                    if let Some(c) = chars.next() {
                        match c {
                            'n' => result.push('\n'),
                            't' => result.push('\t'),
                            'r' => result.push('\r'),
                            _ => result.push(c),
                        }
                    }
                }
                Some(c) => result.push(c),
            }
        }
        Some(result)
    } else {
        None
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn respond_ok(msg_type: &str, extra: &[(&str, &str)]) {
    let mut resp = format!("!{{\"ok\":true,\"type\":\"{}\"", msg_type);
    for (k, v) in extra {
        resp.push_str(&format!(",\"{}\":\"{}\"", k, json_escape(v)));
    }
    resp.push('}');
    print_response(&resp);
}

fn respond_err(msg: &str) {
    print_response(&format!("!{{\"ok\":false,\"error\":\"{}\"}}", json_escape(msg)));
}

fn print_response(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", line);
    let _ = stdout.flush();
}
