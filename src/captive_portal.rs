// Captive portal HTTP route helpers.
//
// Pure functions — no ESP deps — so they're testable on the host.
// The ESP-specific route registration lives in server.rs.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::http::html_escape;

// ---------------------------------------------------------------------------
// Path classification (FR-011, FR-014)
// ---------------------------------------------------------------------------

/// Check if a URI path is an OMI API path that should be excluded from
/// captive portal redirection (FR-011).
///
/// OMI API paths: `/omi`, `/omi/...`, `/omi/ws`.
pub fn is_omi_api_path(path: &str) -> bool {
    path == "/omi" || path.starts_with("/omi/")
}

/// Check if a request should be redirected to the captive portal form (FR-014).
///
/// Returns `true` for non-API GET requests that should be redirected.
/// Portal-specific paths (`/`, `/provision`, `/scan`, `/status`) and
/// OMI API paths are excluded from redirection.
pub fn should_redirect_to_portal(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    // Portal's own routes — never redirect
    if path == "/" || path == "/provision" || path == "/scan" || path == "/status" {
        return false;
    }
    // OMI API paths — excluded from redirect (FR-011)
    if is_omi_api_path(path) {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Redirect response (FR-014)
// ---------------------------------------------------------------------------

/// Render an HTTP 302 redirect body pointing to the provisioning form.
///
/// Returns `(status, headers, body)` for the redirect response.
/// The `portal_ip` is the device's AP IP address (typically "192.168.4.1").
pub fn redirect_to_form(portal_ip: &str) -> (u16, [(&str, String); 1], &'static [u8]) {
    let location = alloc::format!("http://{}/", portal_ip);
    (302, [("Location", location)], b"Redirecting to captive portal...")
}

// ---------------------------------------------------------------------------
// WiFi scan results (GET /scan)
// ---------------------------------------------------------------------------

/// A visible WiFi network from a scan.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct ScannedNetwork {
    pub ssid: String,
    pub rssi: i32,
    pub auth: String,
}

/// Serialize scan results to JSON.
#[cfg(feature = "json")]
pub fn scan_results_json(networks: &[ScannedNetwork]) -> Result<String, serde_json::Error> {
    serde_json::to_string(networks)
}

// ---------------------------------------------------------------------------
// Connection status (GET /status)
// ---------------------------------------------------------------------------

/// Connection attempt status reported to the client after form submission.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub message: Option<String>,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize))]
#[cfg_attr(feature = "json", serde(rename_all = "snake_case"))]
pub enum ConnectionState {
    Idle,
    Connecting,
    Connected,
    Failed,
}

/// Serialize connection status to JSON.
#[cfg(feature = "json")]
pub fn connection_status_json(status: &ConnectionStatus) -> Result<String, serde_json::Error> {
    serde_json::to_string(status)
}

// ---------------------------------------------------------------------------
// Form data parsing (POST /provision)
// ---------------------------------------------------------------------------

/// Parsed provisioning form submission.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisionForm {
    pub credentials: Vec<WifiCredential>,
    pub hostname: Option<String>,
    pub api_key_action: ApiKeyAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WifiCredential {
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiKeyAction {
    Keep,
    Generate,
    Set(String),
}

#[derive(Debug, PartialEq)]
pub enum FormError {
    NoCredentials,
    EmptySsid,
    InvalidEncoding,
}

/// URL-decode a percent-encoded string.
fn url_decode(s: &str) -> Result<String, FormError> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.as_bytes().iter();
    while let Some(&b) = bytes.next() {
        if b == b'+' {
            out.push(b' ');
        } else if b == b'%' {
            let hi = bytes.next().ok_or(FormError::InvalidEncoding)?;
            let lo = bytes.next().ok_or(FormError::InvalidEncoding)?;
            let hex = [*hi, *lo];
            let s = core::str::from_utf8(&hex).map_err(|_| FormError::InvalidEncoding)?;
            let val = u8::from_str_radix(s, 16).map_err(|_| FormError::InvalidEncoding)?;
            out.push(val);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).map_err(|_| FormError::InvalidEncoding)
}

/// Parse URL-encoded form body from POST /provision.
///
/// Expected fields:
/// - `ssid_0`, `password_0`, `ssid_1`, `password_1`, ... (up to `max_aps`)
/// - `hostname` (optional)
/// - `api_key_action` = "keep" | "generate" | "set"
/// - `api_key` (when action is "set")
pub fn parse_provision_form(body: &str, max_aps: usize) -> Result<ProvisionForm, FormError> {
    let mut ssids: Vec<(usize, String)> = Vec::new();
    let mut passwords: Vec<(usize, String)> = Vec::new();
    let mut hostname = None;
    let mut api_key_action_raw = None;
    let mut api_key_value = None;

    for pair in body.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, url_decode(v)?),
            None => continue,
        };
        if let Some(idx_str) = key.strip_prefix("ssid_") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < max_aps {
                    ssids.push((idx, value));
                }
            }
        } else if let Some(idx_str) = key.strip_prefix("password_") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < max_aps {
                    passwords.push((idx, value));
                }
            }
        } else if key == "hostname" {
            if !value.is_empty() {
                hostname = Some(value);
            }
        } else if key == "api_key_action" {
            api_key_action_raw = Some(value);
        } else if key == "api_key" {
            if !value.is_empty() {
                api_key_value = Some(value);
            }
        }
    }

    // Build credentials from matched ssid/password pairs
    let mut credentials = Vec::new();
    for (idx, ssid) in &ssids {
        if ssid.is_empty() {
            continue;
        }
        let password = passwords
            .iter()
            .find(|(i, _)| i == idx)
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        credentials.push(WifiCredential {
            ssid: ssid.clone(),
            password,
        });
    }

    if credentials.is_empty() {
        return Err(FormError::NoCredentials);
    }

    // Validate no empty SSIDs in provided credentials
    if credentials.iter().any(|c| c.ssid.is_empty()) {
        return Err(FormError::EmptySsid);
    }

    let api_key_action = match api_key_action_raw.as_deref() {
        Some("generate") => ApiKeyAction::Generate,
        Some("set") => match api_key_value {
            Some(key) => ApiKeyAction::Set(key),
            None => ApiKeyAction::Keep, // "set" without a value = keep
        },
        _ => ApiKeyAction::Keep,
    };

    Ok(ProvisionForm {
        credentials,
        hostname,
        api_key_action,
    })
}

// ---------------------------------------------------------------------------
// Provisioning form HTML (GET /)
// ---------------------------------------------------------------------------

/// Render the captive portal provisioning form HTML.
///
/// - `max_aps`: maximum number of WiFi AP slots to show (FR-004)
/// - `saved_ssids`: previously saved SSIDs to pre-fill (passwords NOT shown, FR-006)
/// - `hostname`: current hostname to pre-fill
/// - `is_first_setup`: if true, API key is mandatory
/// - `error_message`: optional error from a previous submission attempt
pub fn render_provisioning_form(
    max_aps: usize,
    saved_ssids: &[&str],
    hostname: &str,
    is_first_setup: bool,
    error_message: Option<&str>,
) -> String {
    let mut html = String::with_capacity(2048);
    html.push_str("<!DOCTYPE html><html><head>\
        <meta charset=\"utf-8\">\
        <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
        <title>Device Setup</title>\
        <style>\
        *{box-sizing:border-box;margin:0;padding:0}\
        body{font-family:sans-serif;padding:1em;max-width:480px;margin:0 auto;background:#f5f5f5}\
        h1{margin-bottom:.5em}\
        .field{margin-bottom:1em}\
        label{display:block;font-weight:bold;margin-bottom:.25em}\
        input[type=text],input[type=password],select{width:100%;padding:.5em;border:1px solid #ccc;border-radius:4px}\
        button{padding:.75em 1.5em;background:#0066cc;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:1em}\
        button:hover{background:#0052a3}\
        .error{background:#fee;border:1px solid #c00;padding:.75em;border-radius:4px;margin-bottom:1em;color:#c00}\
        .ap-group{border:1px solid #ddd;padding:.75em;margin-bottom:.5em;border-radius:4px;background:#fff}\
        .ap-group h3{margin-bottom:.5em}\
        #status{display:none;padding:.75em;border-radius:4px;margin-top:1em}\
        </style></head><body>\
        <h1>Device Setup</h1>");

    if let Some(err) = error_message {
        html.push_str("<div class=\"error\">");
        html.push_str(&html_escape(err));
        html.push_str("</div>");
    }

    html.push_str("<form method=\"POST\" action=\"/provision\" id=\"provForm\">");

    // WiFi AP slots
    for i in 0..max_aps {
        let label = if i == 0 { "WiFi Network (required)" } else { "WiFi Network (optional)" };
        let saved = saved_ssids.get(i).copied().unwrap_or("");
        html.push_str("<div class=\"ap-group\"><h3>");
        html.push_str(label);
        html.push_str("</h3><div class=\"field\"><label>SSID</label>\
            <input type=\"text\" name=\"ssid_");
        html.push_str(&alloc::format!("{}", i));
        html.push_str("\" value=\"");
        html.push_str(&html_escape(saved));
        html.push('"');
        if i == 0 {
            html.push_str(" required");
        }
        html.push_str("></div><div class=\"field\"><label>Password</label>\
            <input type=\"password\" name=\"password_");
        html.push_str(&alloc::format!("{}", i));
        html.push_str("\"></div></div>");
    }

    // Hostname
    html.push_str("<div class=\"field\"><label>Hostname</label>\
        <input type=\"text\" name=\"hostname\" value=\"");
    html.push_str(&html_escape(hostname));
    html.push_str("\" placeholder=\"eOMI\"></div>");

    // API key management
    html.push_str("<div class=\"field\"><label>API Key</label>\
        <select name=\"api_key_action\">");
    if !is_first_setup {
        html.push_str("<option value=\"keep\">Keep existing</option>");
    }
    html.push_str("<option value=\"generate\"");
    if is_first_setup {
        html.push_str(" selected");
    }
    html.push_str(">Generate new</option>\
        <option value=\"set\">Set manually</option>\
        </select></div>\
        <div class=\"field\" id=\"apiKeyField\" style=\"display:none\">\
        <label>API Key Value</label>\
        <input type=\"text\" name=\"api_key\" placeholder=\"Enter API key\"></div>");

    html.push_str("<button type=\"submit\">Save &amp; Connect</button></form>\
        <div id=\"status\"></div>\
        <script>\
        var sel=document.querySelector('[name=api_key_action]');\
        var f=document.getElementById('apiKeyField');\
        sel.addEventListener('change',function(){f.style.display=sel.value==='set'?'':'none'});\
        sel.dispatchEvent(new Event('change'));\
        document.getElementById('provForm').addEventListener('submit',function(){\
        var s=document.getElementById('status');\
        s.style.display='block';s.style.background='#ffe';s.style.border='1px solid #cc0';\
        s.textContent='Connecting...';\
        setTimeout(function(){fetch('/status').then(function(r){return r.json()}).then(function(d){\
        if(d.state==='connected'){s.style.background='#efe';s.style.border='1px solid #0c0';\
        s.textContent='Connected! IP: '+(d.ip||'unknown')}\
        else if(d.state==='failed'){s.style.background='#fee';s.style.border='1px solid #c00';\
        s.textContent='Failed: '+(d.message||'unknown error')}\
        else{s.textContent='Status: '+d.state}}).catch(function(){s.textContent='Checking...'})},3000)});\
        </script></body></html>");

    html
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_omi_api_path ---

    #[test]
    fn omi_path_exact() {
        assert!(is_omi_api_path("/omi"));
    }

    #[test]
    fn omi_path_with_subpath() {
        assert!(is_omi_api_path("/omi/Device/Temp"));
    }

    #[test]
    fn omi_path_ws() {
        assert!(is_omi_api_path("/omi/ws"));
    }

    #[test]
    fn omi_path_root_slash() {
        assert!(is_omi_api_path("/omi/"));
    }

    #[test]
    fn non_omi_path() {
        assert!(!is_omi_api_path("/"));
        assert!(!is_omi_api_path("/provision"));
        assert!(!is_omi_api_path("/scan"));
        assert!(!is_omi_api_path("/status"));
        assert!(!is_omi_api_path("/dashboard"));
    }

    #[test]
    fn false_prefix_not_omi() {
        assert!(!is_omi_api_path("/omission"));
        assert!(!is_omi_api_path("/omitted"));
    }

    // --- should_redirect_to_portal ---

    #[test]
    fn redirect_regular_get() {
        assert!(should_redirect_to_portal("GET", "/generate_204"));
        assert!(should_redirect_to_portal("GET", "/hotspot-detect.html"));
        assert!(should_redirect_to_portal("GET", "/favicon.ico"));
        assert!(should_redirect_to_portal("GET", "/random/page"));
    }

    #[test]
    fn no_redirect_portal_paths() {
        assert!(!should_redirect_to_portal("GET", "/"));
        assert!(!should_redirect_to_portal("GET", "/provision"));
        assert!(!should_redirect_to_portal("GET", "/scan"));
        assert!(!should_redirect_to_portal("GET", "/status"));
    }

    #[test]
    fn no_redirect_omi_paths() {
        assert!(!should_redirect_to_portal("GET", "/omi"));
        assert!(!should_redirect_to_portal("GET", "/omi/Device/Temp"));
        assert!(!should_redirect_to_portal("GET", "/omi/ws"));
    }

    #[test]
    fn no_redirect_non_get() {
        assert!(!should_redirect_to_portal("POST", "/random"));
        assert!(!should_redirect_to_portal("PUT", "/something"));
        assert!(!should_redirect_to_portal("DELETE", "/other"));
    }

    // --- url_decode ---

    #[test]
    fn decode_plain() {
        assert_eq!(url_decode("hello").unwrap(), "hello");
    }

    #[test]
    fn decode_plus_to_space() {
        assert_eq!(url_decode("hello+world").unwrap(), "hello world");
    }

    #[test]
    fn decode_percent() {
        assert_eq!(url_decode("hello%20world").unwrap(), "hello world");
        assert_eq!(url_decode("100%25").unwrap(), "100%");
    }

    #[test]
    fn decode_mixed() {
        assert_eq!(url_decode("a%26b+c").unwrap(), "a&b c");
    }

    #[test]
    fn decode_truncated_percent() {
        assert_eq!(url_decode("foo%2"), Err(FormError::InvalidEncoding));
        assert_eq!(url_decode("foo%"), Err(FormError::InvalidEncoding));
    }

    #[test]
    fn decode_invalid_hex() {
        assert_eq!(url_decode("foo%GG"), Err(FormError::InvalidEncoding));
    }

    // --- parse_provision_form ---

    #[test]
    fn parse_basic_form() {
        let body = "ssid_0=MyNetwork&password_0=secret123&hostname=mydevice&api_key_action=generate";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.credentials.len(), 1);
        assert_eq!(form.credentials[0].ssid, "MyNetwork");
        assert_eq!(form.credentials[0].password, "secret123");
        assert_eq!(form.hostname, Some("mydevice".into()));
        assert_eq!(form.api_key_action, ApiKeyAction::Generate);
    }

    #[test]
    fn parse_multiple_aps() {
        let body = "ssid_0=Net1&password_0=pass1&ssid_1=Net2&password_1=pass2&api_key_action=keep";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.credentials.len(), 2);
        assert_eq!(form.credentials[0].ssid, "Net1");
        assert_eq!(form.credentials[1].ssid, "Net2");
    }

    #[test]
    fn parse_skips_empty_ssids() {
        let body = "ssid_0=Net1&password_0=pass1&ssid_1=&password_1=pass2&api_key_action=keep";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.credentials.len(), 1);
        assert_eq!(form.credentials[0].ssid, "Net1");
    }

    #[test]
    fn parse_no_credentials_error() {
        let body = "hostname=test&api_key_action=keep";
        assert_eq!(parse_provision_form(body, 3), Err(FormError::NoCredentials));
    }

    #[test]
    fn parse_all_empty_ssids_error() {
        let body = "ssid_0=&ssid_1=&api_key_action=keep";
        assert_eq!(parse_provision_form(body, 3), Err(FormError::NoCredentials));
    }

    #[test]
    fn parse_set_api_key() {
        let body = "ssid_0=Net&password_0=pass&api_key_action=set&api_key=my-secret-key";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.api_key_action, ApiKeyAction::Set("my-secret-key".into()));
    }

    #[test]
    fn parse_set_without_value_falls_back_to_keep() {
        let body = "ssid_0=Net&password_0=pass&api_key_action=set&api_key=";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.api_key_action, ApiKeyAction::Keep);
    }

    #[test]
    fn parse_url_encoded_values() {
        let body = "ssid_0=My%20Network&password_0=p%26ss%3Dw0rd&api_key_action=keep";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.credentials[0].ssid, "My Network");
        assert_eq!(form.credentials[0].password, "p&ss=w0rd");
    }

    #[test]
    fn parse_respects_max_aps() {
        let body = "ssid_0=A&password_0=a&ssid_1=B&password_1=b&ssid_2=C&password_2=c&api_key_action=keep";
        let form = parse_provision_form(body, 2).unwrap();
        assert_eq!(form.credentials.len(), 2);
        // ssid_2 should be ignored
    }

    #[test]
    fn parse_missing_password_defaults_empty() {
        let body = "ssid_0=OpenNet&api_key_action=keep";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.credentials[0].password, "");
    }

    #[test]
    fn parse_default_api_key_action() {
        let body = "ssid_0=Net&password_0=pass";
        let form = parse_provision_form(body, 3).unwrap();
        assert_eq!(form.api_key_action, ApiKeyAction::Keep);
    }

    // --- render_provisioning_form ---

    #[test]
    fn form_contains_required_elements() {
        let html = render_provisioning_form(3, &[], "eOMI", true, None);
        assert!(html.contains("<form method=\"POST\" action=\"/provision\""));
        assert!(html.contains("name=\"ssid_0\""));
        assert!(html.contains("name=\"password_0\""));
        assert!(html.contains("name=\"ssid_1\""));
        assert!(html.contains("name=\"ssid_2\""));
        assert!(html.contains("name=\"hostname\""));
        assert!(html.contains("name=\"api_key_action\""));
        assert!(html.contains("Device Setup"));
    }

    #[test]
    fn form_prefills_saved_ssids() {
        let html = render_provisioning_form(3, &["SavedNet"], "myhost", false, None);
        assert!(html.contains("value=\"SavedNet\""));
        assert!(html.contains("value=\"myhost\""));
    }

    #[test]
    fn form_escapes_html_in_values() {
        let html = render_provisioning_form(1, &["<script>"], "h\"ost", false, None);
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("h&quot;ost"));
        // The SSID value must be escaped — check it doesn't appear raw in a value attr
        assert!(!html.contains("value=\"<script>\""));
    }

    #[test]
    fn form_shows_error_message() {
        let html = render_provisioning_form(1, &[], "eOMI", true, Some("Connection failed"));
        assert!(html.contains("Connection failed"));
        assert!(html.contains("class=\"error\""));
    }

    #[test]
    fn form_first_setup_no_keep_option() {
        let html = render_provisioning_form(1, &[], "eOMI", true, None);
        assert!(!html.contains("value=\"keep\""));
        assert!(html.contains("value=\"generate\""));
    }

    #[test]
    fn form_reprovisioning_has_keep_option() {
        let html = render_provisioning_form(1, &["Net"], "eOMI", false, None);
        assert!(html.contains("value=\"keep\""));
    }

    // --- ConnectionStatus / ScannedNetwork JSON ---

    #[cfg(feature = "json")]
    #[test]
    fn scan_results_json_basic() {
        let networks = vec![
            ScannedNetwork { ssid: "Home".into(), rssi: -45, auth: "WPA2".into() },
            ScannedNetwork { ssid: "Guest".into(), rssi: -72, auth: "Open".into() },
        ];
        let json = scan_results_json(&networks).unwrap();
        assert!(json.contains("\"Home\""));
        assert!(json.contains("-45"));
        assert!(json.contains("\"Guest\""));
        assert!(json.contains("\"Open\""));
    }

    #[cfg(feature = "json")]
    #[test]
    fn connection_status_json_connected() {
        let status = ConnectionStatus {
            state: ConnectionState::Connected,
            message: None,
            ip: Some("192.168.1.100".into()),
        };
        let json = connection_status_json(&status).unwrap();
        assert!(json.contains("\"connected\""));
        assert!(json.contains("192.168.1.100"));
        assert!(!json.contains("message"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn connection_status_json_failed() {
        let status = ConnectionStatus {
            state: ConnectionState::Failed,
            message: Some("Wrong password".into()),
            ip: None,
        };
        let json = connection_status_json(&status).unwrap();
        assert!(json.contains("\"failed\""));
        assert!(json.contains("Wrong password"));
        assert!(!json.contains("\"ip\""));
    }

    #[cfg(feature = "json")]
    #[test]
    fn connection_status_json_idle() {
        let status = ConnectionStatus {
            state: ConnectionState::Idle,
            message: None,
            ip: None,
        };
        let json = connection_status_json(&status).unwrap();
        assert!(json.contains("\"idle\""));
    }

    // --- redirect_to_form ---

    #[test]
    fn redirect_response() {
        let (status, headers, body) = redirect_to_form("192.168.4.1");
        assert_eq!(status, 302);
        assert_eq!(headers[0].0, "Location");
        assert_eq!(headers[0].1, "http://192.168.4.1/");
        assert!(!body.is_empty());
    }
}
