//! Peripheral protocol bus drivers (UART, SPI) for GPIO pins.
//!
//! Platform-independent types (InfoItem builders) live here.
//! Encoding types are in [`super::encoding`].
//! ESP-specific drivers are in submodules gated on `cfg(feature = "esp")`.

#[cfg(feature = "esp")]
pub mod i2c;
#[cfg(feature = "esp")]
pub mod uart;
#[cfg(feature = "esp")]
pub mod spi;

pub use super::encoding::DataEncoding;

use crate::odf::{InfoItem, OmiValue};
use std::collections::BTreeMap;

/// Ring buffer capacity for peripheral RX/TX InfoItems.
const PERIPHERAL_CAPACITY: usize = 20;

/// Decode a TX value to raw bytes using the given encoding (FR-009a).
pub fn decode_tx_data(v: &OmiValue, encoding: DataEncoding) -> Result<Vec<u8>, String> {
    let s = match v {
        OmiValue::Str(s) => s.as_str(),
        OmiValue::Number(n) => {
            // Use ryu instead of n.to_string() to avoid pulling in core::fmt's
            // float formatting machinery (~10-20 KB on embedded targets).
            let mut buf = ryu::Buffer::new();
            let s = buf.format(*n);
            // Strip trailing ".0" for whole numbers (compact form for wire TX).
            let s = s.strip_suffix(".0").unwrap_or(s);
            return Ok(s.as_bytes().to_vec());
        }
        OmiValue::Bool(b) => return Ok(if *b { b"1".to_vec() } else { b"0".to_vec() }),
        OmiValue::Null => return Ok(Vec::new()),
    };
    encoding.decode(s).map_err(|e| e.0)
}

/// Encode raw bytes to an OmiValue string using the given encoding.
pub fn encode_rx_data(data: &[u8], encoding: DataEncoding) -> OmiValue {
    OmiValue::Str(encoding.encode(data))
}

/// Read the TX encoding from an InfoItem's metadata (FR-009a).
///
/// Returns the encoding stored in the `tx_encoding` metadata key, or
/// [`DataEncoding::String`] if no encoding is set.
pub fn tx_encoding_from_meta(item: &InfoItem) -> DataEncoding {
    item.meta
        .as_ref()
        .and_then(|m| m.get("tx_encoding"))
        .and_then(|v| match v {
            OmiValue::Str(s) => s.parse::<DataEncoding>().ok(),
            _ => None,
        })
        .unwrap_or(DataEncoding::String)
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

/// Peripheral protocol type for build-time configuration (FR-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeripheralProtocol {
    I2C,
    UART,
    SPI,
}

impl PeripheralProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::I2C => "I2C",
            Self::UART => "UART",
            Self::SPI => "SPI",
        }
    }
}

/// Build-time configuration for a single peripheral bus (FR-008).
#[derive(Debug, Clone)]
pub struct PeripheralConfig {
    pub name: String,
    pub protocol: PeripheralProtocol,
}

impl PeripheralConfig {
    pub fn new(name: impl Into<String>, protocol: PeripheralProtocol) -> Self {
        Self {
            name: name.into(),
            protocol,
        }
    }
}

/// Parse the `PERIPHERALS` build-time env var into configs.
///
/// Format: comma-separated `PROTOCOL:NAME` pairs, e.g. `"I2C:GPIO21,UART:GPIO16"`.
/// Returns an empty vec for empty or missing input.
pub fn parse_peripherals(s: &str) -> Vec<PeripheralConfig> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            let (proto_str, name) = entry.split_once(':')?;
            let protocol = match proto_str.trim().to_uppercase().as_str() {
                "I2C" => PeripheralProtocol::I2C,
                "UART" => PeripheralProtocol::UART,
                "SPI" => PeripheralProtocol::SPI,
                _ => return None,
            };
            Some(PeripheralConfig::new(name.trim(), protocol))
        })
        .collect()
}

/// Build RX/TX InfoItems for all configured peripherals (FR-008, FR-009).
///
/// Returns `(name, InfoItem)` pairs to be added directly to the device root
/// object, consistent with how GPIO items are placed (FR-002). Each pair
/// follows the `{name}_{protocol}_RX` / `{name}_{protocol}_TX` naming.
///
/// This is the platform-independent tree builder. ESP-specific drivers attach
/// to these tree paths at runtime.
pub fn build_peripheral_items_all(configs: &[PeripheralConfig]) -> Vec<(String, InfoItem)> {
    configs
        .iter()
        .flat_map(|c| build_peripheral_items(&c.name, c.protocol.as_str()))
        .collect()
}

// --- ESP-only PeripheralManager ---

#[cfg(feature = "esp")]
use crate::odf::{ObjectTree, PathTarget, PathTargetMut};

/// Manages peripheral protocol buses (UART, SPI, I2C) and synchronises their
/// InfoItems with the O-DF tree (FR-008, FR-009, FR-010).
#[cfg(feature = "esp")]
pub struct PeripheralManager {
    uart_buses: Vec<uart::UartBus>,
    spi_buses: Vec<spi::SpiBus>,
    i2c_buses: Vec<i2c::I2cBus>,
}

#[cfg(feature = "esp")]
impl PeripheralManager {
    pub fn new() -> Self {
        Self {
            uart_buses: Vec::new(),
            spi_buses: Vec::new(),
            i2c_buses: Vec::new(),
        }
    }

    pub fn add_uart(&mut self, bus: uart::UartBus) {
        self.uart_buses.push(bus);
    }

    pub fn add_spi(&mut self, bus: spi::SpiBus) {
        self.spi_buses.push(bus);
    }

    pub fn add_i2c(&mut self, bus: i2c::I2cBus) {
        self.i2c_buses.push(bus);
    }

    /// Scan all I2C buses for connected devices (FR-010, SC-005).
    ///
    /// Should be called once at boot before `register_tree_items`.
    pub fn scan_i2c(&mut self) {
        for bus in &mut self.i2c_buses {
            bus.scan();
        }
    }

    /// Register all peripheral RX/TX InfoItems in the O-DF tree.
    ///
    /// For I2C buses, also registers discovered devices as child Objects (FR-010).
    pub fn register_tree_items(&self, tree: &mut ObjectTree) {
        for uart in &self.uart_buses {
            register_bus_items(tree, &uart.rx_path, &uart.tx_path, "UART");
        }
        for spi in &self.spi_buses {
            register_bus_items(tree, &spi.rx_path, &spi.tx_path, "SPI");
        }
        for i2c_bus in &self.i2c_buses {
            register_bus_items(tree, &i2c_bus.rx_path, &i2c_bus.tx_path, "I2C");
            i2c_bus.register_discovered(tree);
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
        for i2c_bus in &mut self.i2c_buses {
            i2c_bus.poll_rx(tree, now);
            i2c_bus.sync_tx(tree);
        }
    }

    pub fn has_buses(&self) -> bool {
        !self.uart_buses.is_empty() || !self.spi_buses.is_empty() || !self.i2c_buses.is_empty()
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

    // --- tx_encoding_from_meta ---

    #[test]
    fn tx_encoding_default_when_no_meta() {
        let item = InfoItem::new(10);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::String);
    }

    #[test]
    fn tx_encoding_default_when_no_key() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("protocol".into(), OmiValue::Str("UART".into()));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::String);
    }

    #[test]
    fn tx_encoding_hex_from_meta() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("tx_encoding".into(), OmiValue::Str("hex".into()));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::Hex);
    }

    #[test]
    fn tx_encoding_base64_from_meta() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("tx_encoding".into(), OmiValue::Str("base64".into()));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::Base64);
    }

    #[test]
    fn tx_encoding_string_from_meta() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("tx_encoding".into(), OmiValue::Str("string".into()));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::String);
    }

    #[test]
    fn tx_encoding_invalid_falls_back_to_string() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("tx_encoding".into(), OmiValue::Str("unknown".into()));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::String);
    }

    #[test]
    fn tx_encoding_non_string_value_falls_back() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("tx_encoding".into(), OmiValue::Number(42.0));
        item.meta = Some(meta);
        assert_eq!(tx_encoding_from_meta(&item), DataEncoding::String);
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
    fn tx_number_fractional() {
        let v = OmiValue::Number(3.14);
        let bytes = decode_tx_data(&v, DataEncoding::String).unwrap();
        assert_eq!(bytes, b"3.14");
    }

    #[test]
    fn tx_number_negative() {
        let v = OmiValue::Number(-1.5);
        let bytes = decode_tx_data(&v, DataEncoding::String).unwrap();
        assert_eq!(bytes, b"-1.5");
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
    fn peripheral_items_i2c_names() {
        let items = build_peripheral_items("GPIO21", "I2C");
        assert_eq!(items[0].0, "GPIO21_I2C_RX");
        assert_eq!(items[1].0, "GPIO21_I2C_TX");
    }

    #[test]
    fn peripheral_items_custom_name() {
        let items = build_peripheral_items("Serial1", "UART");
        assert_eq!(items[0].0, "Serial1_UART_RX");
        assert_eq!(items[1].0, "Serial1_UART_TX");
    }

    // --- parse_peripherals ---

    #[test]
    fn parse_empty_string() {
        assert!(parse_peripherals("").is_empty());
    }

    #[test]
    fn parse_single_i2c() {
        let configs = parse_peripherals("I2C:GPIO21");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "GPIO21");
        assert_eq!(configs[0].protocol, PeripheralProtocol::I2C);
    }

    #[test]
    fn parse_multiple() {
        let configs = parse_peripherals("I2C:GPIO21,UART:GPIO16,SPI:GPIO18");
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].protocol, PeripheralProtocol::I2C);
        assert_eq!(configs[1].protocol, PeripheralProtocol::UART);
        assert_eq!(configs[2].protocol, PeripheralProtocol::SPI);
    }

    #[test]
    fn parse_case_insensitive() {
        let configs = parse_peripherals("i2c:GPIO21,uart:GPIO16");
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].protocol, PeripheralProtocol::I2C);
        assert_eq!(configs[1].protocol, PeripheralProtocol::UART);
    }

    #[test]
    fn parse_with_spaces() {
        let configs = parse_peripherals(" I2C : GPIO21 , UART : GPIO16 ");
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "GPIO21");
        assert_eq!(configs[1].name, "GPIO16");
    }

    #[test]
    fn parse_unknown_protocol_skipped() {
        let configs = parse_peripherals("CAN:GPIO5,I2C:GPIO21");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].protocol, PeripheralProtocol::I2C);
    }

    #[test]
    fn parse_invalid_entry_skipped() {
        let configs = parse_peripherals("NOCOLON,I2C:GPIO21");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "GPIO21");
    }

    // --- PeripheralProtocol ---

    #[test]
    fn protocol_as_str() {
        assert_eq!(PeripheralProtocol::I2C.as_str(), "I2C");
        assert_eq!(PeripheralProtocol::UART.as_str(), "UART");
        assert_eq!(PeripheralProtocol::SPI.as_str(), "SPI");
    }

    // --- PeripheralConfig ---

    #[test]
    fn peripheral_config_new() {
        let cfg = PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C);
        assert_eq!(cfg.name, "GPIO21");
        assert_eq!(cfg.protocol, PeripheralProtocol::I2C);
    }

    // --- build_peripheral_items_all ---

    #[test]
    fn build_all_empty_configs() {
        let items = build_peripheral_items_all(&[]);
        assert!(items.is_empty());
    }

    #[test]
    fn build_all_single_i2c() {
        let configs = vec![PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C)];
        let items = build_peripheral_items_all(&configs);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "GPIO21_I2C_RX");
        assert_eq!(items[1].0, "GPIO21_I2C_TX");
    }

    #[test]
    fn build_all_multiple_protocols() {
        let configs = vec![
            PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C),
            PeripheralConfig::new("GPIO16", PeripheralProtocol::UART),
            PeripheralConfig::new("GPIO18", PeripheralProtocol::SPI),
        ];
        let items = build_peripheral_items_all(&configs);
        assert_eq!(items.len(), 6); // 3 protocols * 2 (RX + TX)
        let names: Vec<&str> = items.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"GPIO21_I2C_RX"));
        assert!(names.contains(&"GPIO21_I2C_TX"));
        assert!(names.contains(&"GPIO16_UART_RX"));
        assert!(names.contains(&"GPIO16_UART_TX"));
        assert!(names.contains(&"GPIO18_SPI_RX"));
        assert!(names.contains(&"GPIO18_SPI_TX"));
    }

    #[test]
    fn build_all_i2c_rx_not_writable() {
        let configs = vec![PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C)];
        let items = build_peripheral_items_all(&configs);
        let rx = &items[0].1;
        assert!(!rx.is_writable());
        let meta = rx.meta.as_ref().unwrap();
        assert_eq!(meta.get("protocol"), Some(&OmiValue::Str("I2C".into())));
        assert_eq!(meta.get("mode"), Some(&OmiValue::Str("i2c_rx".into())));
    }

    #[test]
    fn build_all_i2c_tx_writable() {
        let configs = vec![PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C)];
        let items = build_peripheral_items_all(&configs);
        let tx = &items[1].1;
        assert!(tx.is_writable());
        let meta = tx.meta.as_ref().unwrap();
        assert_eq!(meta.get("protocol"), Some(&OmiValue::Str("I2C".into())));
        assert_eq!(meta.get("mode"), Some(&OmiValue::Str("i2c_tx".into())));
    }

    #[test]
    fn build_all_in_object_tree() {
        use crate::odf::{Object, ObjectTree, PathTarget};

        let configs = vec![PeripheralConfig::new("GPIO21", PeripheralProtocol::I2C)];
        let items = build_peripheral_items_all(&configs);

        let mut device = Object::new("Device");
        for (name, item) in items {
            device.add_item(name, item);
        }

        let mut tree = ObjectTree::new();
        tree.insert_root(device);

        // RX is discoverable directly under device root
        match tree.resolve("/Device/GPIO21_I2C_RX") {
            Ok(PathTarget::InfoItem(item)) => {
                assert!(!item.is_writable());
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }

        // TX is writable directly under device root
        match tree.resolve("/Device/GPIO21_I2C_TX") {
            Ok(PathTarget::InfoItem(item)) => {
                assert!(item.is_writable());
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
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
