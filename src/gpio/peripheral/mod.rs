//! Peripheral protocol bus drivers (UART, SPI) for GPIO pins.
//!
//! Platform-independent types (encoding, InfoItem builders) live here.
//! ESP-specific drivers are in submodules gated on `cfg(feature = "esp")`.

#[cfg(feature = "esp")]
pub mod uart;
#[cfg(feature = "esp")]
pub mod spi;

use crate::odf::{InfoItem, OmiValue};
use std::collections::BTreeMap;

/// Ring buffer capacity for peripheral RX/TX InfoItems.
const PERIPHERAL_CAPACITY: usize = 20;

/// Data encoding for peripheral protocol TX/RX data (FR-009a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataEncoding {
    /// UTF-8 string (default).
    String,
    /// Hex-encoded binary data.
    Hex,
    /// Base64-encoded binary data.
    Base64,
}

impl DataEncoding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Hex => "hex",
            Self::Base64 => "base64",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "string" => Some(Self::String),
            "hex" => Some(Self::Hex),
            "base64" => Some(Self::Base64),
            _ => None,
        }
    }
}

/// Decode a TX value to raw bytes using the given encoding (FR-009a).
pub fn decode_tx_data(v: &OmiValue, encoding: DataEncoding) -> Result<Vec<u8>, String> {
    let s = match v {
        OmiValue::Str(s) => s.as_str(),
        OmiValue::Number(n) => return Ok(n.to_string().into_bytes()),
        OmiValue::Bool(b) => return Ok(if *b { b"1".to_vec() } else { b"0".to_vec() }),
        OmiValue::Null => return Ok(Vec::new()),
    };
    match encoding {
        DataEncoding::String => Ok(s.as_bytes().to_vec()),
        DataEncoding::Hex => decode_hex(s),
        DataEncoding::Base64 => decode_base64(s),
    }
}

/// Encode raw bytes to an OmiValue string using the given encoding.
pub fn encode_rx_data(data: &[u8], encoding: DataEncoding) -> OmiValue {
    match encoding {
        DataEncoding::String => OmiValue::Str(String::from_utf8_lossy(data).into_owned()),
        DataEncoding::Hex => OmiValue::Str(encode_hex(data)),
        DataEncoding::Base64 => OmiValue::Str(encode_base64(data)),
    }
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex string must have even length".into());
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|_| format!("invalid hex at position {}", i))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn encode_hex(data: &[u8]) -> String {
    let mut s = std::string::String::with_capacity(data.len() * 2);
    for b in data {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.bytes() {
        let val = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\n' | b'\r' | b' ' => continue,
            _ => return Err(format!("invalid base64 character: {}", c as char)),
        };
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(bytes)
}

fn encode_base64(data: &[u8]) -> String {
    let mut s = std::string::String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        s.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        s.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            s.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            s.push('=');
        }
        if chunk.len() > 2 {
            s.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            s.push('=');
        }
    }
    s
}

/// Build RX and TX InfoItems for a peripheral bus (FR-009).
///
/// Returns `[(rx_name, rx_item), (tx_name, tx_item)]`.
pub fn build_peripheral_items(name: &str, protocol: &str) -> Vec<(String, InfoItem)> {
    let rx_name = format!("{}_{}_RX", name, protocol);
    let tx_name = format!("{}_{}_TX", name, protocol);

    let mut rx_item = InfoItem::new(PERIPHERAL_CAPACITY);
    let mut rx_meta = BTreeMap::new();
    rx_meta.insert("mode".into(), OmiValue::Str(format!("{}_rx", protocol.to_lowercase())));
    rx_meta.insert("protocol".into(), OmiValue::Str(protocol.into()));
    rx_item.meta = Some(rx_meta);

    let mut tx_item = InfoItem::new(PERIPHERAL_CAPACITY);
    let mut tx_meta = BTreeMap::new();
    tx_meta.insert("mode".into(), OmiValue::Str(format!("{}_tx", protocol.to_lowercase())));
    tx_meta.insert("protocol".into(), OmiValue::Str(protocol.into()));
    tx_meta.insert("writable".into(), OmiValue::Bool(true));
    tx_item.meta = Some(tx_meta);

    vec![(rx_name, rx_item), (tx_name, tx_item)]
}

// --- ESP-only PeripheralManager ---

#[cfg(feature = "esp")]
use crate::odf::{ObjectTree, PathTarget, PathTargetMut};

/// Manages peripheral protocol buses (UART, SPI) and synchronises their
/// InfoItems with the O-DF tree (FR-008, FR-009).
#[cfg(feature = "esp")]
pub struct PeripheralManager {
    uart_buses: Vec<uart::UartBus>,
    spi_buses: Vec<spi::SpiBus>,
}

#[cfg(feature = "esp")]
impl PeripheralManager {
    pub fn new() -> Self {
        Self {
            uart_buses: Vec::new(),
            spi_buses: Vec::new(),
        }
    }

    pub fn add_uart(&mut self, bus: uart::UartBus) {
        self.uart_buses.push(bus);
    }

    pub fn add_spi(&mut self, bus: spi::SpiBus) {
        self.spi_buses.push(bus);
    }

    /// Register all peripheral RX/TX InfoItems in the O-DF tree.
    pub fn register_tree_items(&self, tree: &mut ObjectTree) {
        for uart in &self.uart_buses {
            register_bus_items(tree, &uart.rx_path, &uart.tx_path, "UART");
        }
        for spi in &self.spi_buses {
            register_bus_items(tree, &spi.rx_path, &spi.tx_path, "SPI");
        }
    }

    /// Poll all peripheral buses: read RX data and sync TX writes.
    ///
    /// Called from the main loop at 100ms intervals.
    pub fn poll(&mut self, tree: &mut ObjectTree, now: f64) {
        for uart in &mut self.uart_buses {
            uart.poll_rx(tree, now);
            uart.sync_tx(tree);
        }
        for spi_bus in &mut self.spi_buses {
            spi_bus.poll(tree, now);
        }
    }

    pub fn has_buses(&self) -> bool {
        !self.uart_buses.is_empty() || !self.spi_buses.is_empty()
    }
}

#[cfg(feature = "esp")]
fn register_bus_items(tree: &mut ObjectTree, rx_path: &str, tx_path: &str, protocol: &str) {
    let proto_lower = protocol.to_lowercase();

    // RX InfoItem (read-only)
    if let Err(e) = tree.write_value(rx_path, OmiValue::Str(String::new()), None) {
        log::warn!("Failed to init {} RX InfoItem at {}: {}", protocol, rx_path, e);
    } else if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(rx_path) {
        item.type_uri = Some(format!("omi:gpio:{}_rx", proto_lower));
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("mode".into(), OmiValue::Str(format!("{}_rx", proto_lower)));
        meta.insert("protocol".into(), OmiValue::Str(protocol.into()));
    }

    // TX InfoItem (writable)
    if let Err(e) = tree.write_value(tx_path, OmiValue::Str(String::new()), None) {
        log::warn!("Failed to init {} TX InfoItem at {}: {}", protocol, tx_path, e);
    } else if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(tx_path) {
        item.type_uri = Some(format!("omi:gpio:{}_tx", proto_lower));
        let meta = item.meta.get_or_insert_with(BTreeMap::new);
        meta.insert("mode".into(), OmiValue::Str(format!("{}_tx", proto_lower)));
        meta.insert("protocol".into(), OmiValue::Str(protocol.into()));
        meta.insert("writable".into(), OmiValue::Bool(true));
    }

    log::info!("{} InfoItems registered: RX={}, TX={} (writable)", protocol, rx_path, tx_path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    // --- DataEncoding ---

    #[test]
    fn encoding_as_str() {
        assert_eq!(DataEncoding::String.as_str(), "string");
        assert_eq!(DataEncoding::Hex.as_str(), "hex");
        assert_eq!(DataEncoding::Base64.as_str(), "base64");
    }

    #[test]
    fn encoding_parse() {
        assert_eq!(DataEncoding::parse("string"), Some(DataEncoding::String));
        assert_eq!(DataEncoding::parse("hex"), Some(DataEncoding::Hex));
        assert_eq!(DataEncoding::parse("base64"), Some(DataEncoding::Base64));
        assert_eq!(DataEncoding::parse("unknown"), None);
    }

    // --- Hex encoding ---

    #[test]
    fn decode_hex_hello() {
        assert_eq!(decode_hex("48656C6C6F").unwrap(), b"Hello");
    }

    #[test]
    fn decode_hex_lowercase() {
        assert_eq!(decode_hex("48656c6c6f").unwrap(), b"Hello");
    }

    #[test]
    fn decode_hex_empty() {
        assert_eq!(decode_hex("").unwrap(), b"");
    }

    #[test]
    fn decode_hex_odd_length() {
        assert!(decode_hex("ABC").is_err());
    }

    #[test]
    fn decode_hex_invalid_chars() {
        assert!(decode_hex("ZZZZ").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        let data = b"Hello, World!";
        let hex = encode_hex(data);
        assert_eq!(decode_hex(&hex).unwrap(), data);
    }

    // --- Base64 encoding ---

    #[test]
    fn decode_base64_hello() {
        assert_eq!(decode_base64("SGVsbG8=").unwrap(), b"Hello");
    }

    #[test]
    fn decode_base64_no_padding() {
        assert_eq!(decode_base64("SGVsbG8").unwrap(), b"Hello");
    }

    #[test]
    fn decode_base64_aqid() {
        // AQID decodes to [0x01, 0x02, 0x03]
        assert_eq!(decode_base64("AQID").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn decode_base64_empty() {
        assert_eq!(decode_base64("").unwrap(), b"");
    }

    #[test]
    fn decode_base64_invalid_char() {
        assert!(decode_base64("@@@").is_err());
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello, World!";
        let b64 = encode_base64(data);
        assert_eq!(decode_base64(&b64).unwrap(), data);
    }

    #[test]
    fn base64_single_byte() {
        let data = b"\x01";
        let b64 = encode_base64(data);
        assert_eq!(decode_base64(&b64).unwrap(), data);
    }

    #[test]
    fn base64_two_bytes() {
        let data = b"\x01\x02";
        let b64 = encode_base64(data);
        assert_eq!(decode_base64(&b64).unwrap(), data);
    }

    // --- decode_tx_data ---

    #[test]
    fn tx_string_encoding() {
        let v = OmiValue::Str("Hello".into());
        assert_eq!(decode_tx_data(&v, DataEncoding::String).unwrap(), b"Hello");
    }

    #[test]
    fn tx_hex_encoding() {
        let v = OmiValue::Str("48656C6C6F".into());
        assert_eq!(decode_tx_data(&v, DataEncoding::Hex).unwrap(), b"Hello");
    }

    #[test]
    fn tx_base64_encoding() {
        let v = OmiValue::Str("AQID".into());
        assert_eq!(decode_tx_data(&v, DataEncoding::Base64).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn tx_number_any_encoding() {
        let v = OmiValue::Number(42.0);
        let bytes = decode_tx_data(&v, DataEncoding::String).unwrap();
        assert_eq!(bytes, b"42");
    }

    #[test]
    fn tx_bool_true() {
        assert_eq!(decode_tx_data(&OmiValue::Bool(true), DataEncoding::String).unwrap(), b"1");
    }

    #[test]
    fn tx_bool_false() {
        assert_eq!(decode_tx_data(&OmiValue::Bool(false), DataEncoding::String).unwrap(), b"0");
    }

    #[test]
    fn tx_null() {
        assert_eq!(decode_tx_data(&OmiValue::Null, DataEncoding::String).unwrap(), b"");
    }

    // --- encode_rx_data ---

    #[test]
    fn rx_string_encoding() {
        let data = b"Hello";
        assert_eq!(encode_rx_data(data, DataEncoding::String), OmiValue::Str("Hello".into()));
    }

    #[test]
    fn rx_hex_encoding() {
        let data = b"Hello";
        assert_eq!(encode_rx_data(data, DataEncoding::Hex), OmiValue::Str("48656c6c6f".into()));
    }

    #[test]
    fn rx_base64_encoding() {
        let data = &[1u8, 2, 3];
        assert_eq!(encode_rx_data(data, DataEncoding::Base64), OmiValue::Str("AQID".into()));
    }

    #[test]
    fn rx_string_invalid_utf8() {
        let data = &[0xFF, 0xFE];
        match encode_rx_data(data, DataEncoding::String) {
            OmiValue::Str(s) => assert!(s.contains('\u{FFFD}')),
            _ => panic!("expected Str"),
        }
    }

    // --- build_peripheral_items ---

    #[test]
    fn peripheral_items_names() {
        let items = build_peripheral_items("GPIO16", "UART");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "GPIO16_UART_RX");
        assert_eq!(items[1].0, "GPIO16_UART_TX");
    }

    #[test]
    fn peripheral_items_spi_names() {
        let items = build_peripheral_items("GPIO18", "SPI");
        assert_eq!(items[0].0, "GPIO18_SPI_RX");
        assert_eq!(items[1].0, "GPIO18_SPI_TX");
    }

    #[test]
    fn peripheral_rx_not_writable() {
        let items = build_peripheral_items("GPIO16", "UART");
        let rx = &items[0].1;
        assert!(!rx.is_writable());
        let meta = rx.meta.as_ref().unwrap();
        assert_eq!(meta.get("protocol"), Some(&OmiValue::Str("UART".into())));
        assert_eq!(meta.get("mode"), Some(&OmiValue::Str("uart_rx".into())));
    }

    #[test]
    fn peripheral_tx_writable() {
        let items = build_peripheral_items("GPIO16", "UART");
        let tx = &items[1].1;
        assert!(tx.is_writable());
        let meta = tx.meta.as_ref().unwrap();
        assert_eq!(meta.get("protocol"), Some(&OmiValue::Str("UART".into())));
        assert_eq!(meta.get("mode"), Some(&OmiValue::Str("uart_tx".into())));
        assert_eq!(meta.get("writable"), Some(&OmiValue::Bool(true)));
    }

    #[test]
    fn peripheral_items_custom_name() {
        let items = build_peripheral_items("Serial1", "UART");
        assert_eq!(items[0].0, "Serial1_UART_RX");
        assert_eq!(items[1].0, "Serial1_UART_TX");
    }

    // --- Integration with ObjectTree ---

    #[test]
    fn peripheral_items_in_tree() {
        use crate::odf::{Object, ObjectTree, PathTarget};

        let items = build_peripheral_items("GPIO16", "UART");
        let mut device = Object::new("Device");
        for (name, item) in items {
            device.add_item(name, item);
        }

        let mut tree = ObjectTree::new();
        tree.insert_root(device);

        // RX is discoverable and read-only
        match tree.resolve("/Device/GPIO16_UART_RX") {
            Ok(PathTarget::InfoItem(item)) => {
                assert!(!item.is_writable());
                let meta = item.meta.as_ref().unwrap();
                assert_eq!(meta.get("protocol"), Some(&OmiValue::Str("UART".into())));
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }

        // TX is discoverable and writable
        match tree.resolve("/Device/GPIO16_UART_TX") {
            Ok(PathTarget::InfoItem(item)) => {
                assert!(item.is_writable());
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }
}
