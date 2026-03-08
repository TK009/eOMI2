//! I2C peripheral bus driver for ESP32 (FR-008, FR-009, FR-010).
//!
//! Wraps the ESP-IDF I2C master driver. At boot, scans addresses 0x08–0x77
//! and adds discovered devices as child Objects in the O-DF tree.
//! RX/TX InfoItems allow raw I2C read/write via the O-DF interface.

use esp_idf_svc::hal::gpio::{InputPin, OutputPin};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::EspError;
use log::{info, warn};

use crate::odf::{Object, ObjectTree, OmiValue, PathTarget};

use super::{decode_tx_data, encode_rx_data, tx_encoding_from_meta, DataEncoding};

/// Default I2C clock frequency (100 kHz standard mode).
pub const DEFAULT_FREQ_HZ: u32 = 100_000;

/// First scannable 7-bit I2C address.
const SCAN_ADDR_START: u8 = 0x08;

/// Last scannable 7-bit I2C address (inclusive).
const SCAN_ADDR_END: u8 = 0x77;

/// Timeout for scan probe in ticks (~10ms).
const SCAN_TIMEOUT_TICKS: u32 = 10;

/// Default read length for RX polling (bytes).
const DEFAULT_RX_LEN: usize = 32;

/// Configuration for an I2C peripheral bus.
#[derive(Debug, Clone)]
pub struct I2cConfig2 {
    pub freq_hz: u32,
    pub name: String,
}

impl I2cConfig2 {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            freq_hz: DEFAULT_FREQ_HZ,
            name: name.into(),
        }
    }

    pub fn with_freq(mut self, freq_hz: u32) -> Self {
        self.freq_hz = freq_hz;
        self
    }
}

/// An I2C bus managing discovery, RX polling, and TX writes.
pub struct I2cBus {
    driver: I2cDriver<'static>,
    pub(crate) rx_path: String,
    pub(crate) tx_path: String,
    pub(crate) device_path: String,
    pub(crate) name: String,
    last_tx: Option<(OmiValue, Option<f64>)>,
    discovered_addrs: Vec<u8>,
}

impl I2cBus {
    /// Create a new I2C bus driver.
    ///
    /// `device_path` is the O-DF device root (e.g. `"/MyDevice"`).
    /// The RX/TX InfoItem paths are derived as `{device_path}/{name}_I2C_RX`
    /// and `{device_path}/{name}_I2C_TX`.
    pub fn new<I2C: esp_idf_svc::hal::i2c::I2c>(
        device_path: &str,
        config: &I2cConfig2,
        i2c: impl Peripheral<P = I2C> + 'static,
        sda: impl Peripheral<P = impl InputPin + OutputPin> + 'static,
        scl: impl Peripheral<P = impl InputPin + OutputPin> + 'static,
    ) -> Result<Self, EspError> {
        let i2c_config = I2cConfig::new().baudrate(Hertz(config.freq_hz));
        let driver = I2cDriver::new(i2c, sda, scl, &i2c_config)?;

        let rx_path = format!("{}/{}_I2C_RX", device_path, config.name);
        let tx_path = format!("{}/{}_I2C_TX", device_path, config.name);

        info!(
            "I2C bus created: {} ({}Hz) RX={} TX={}",
            config.name, config.freq_hz, rx_path, tx_path
        );

        Ok(Self {
            driver,
            rx_path,
            tx_path,
            device_path: device_path.to_string(),
            name: config.name.clone(),
            last_tx: None,
            discovered_addrs: Vec::new(),
        })
    }

    /// Scan all 7-bit I2C addresses (0x08–0x77) for connected devices (FR-010).
    ///
    /// Returns a list of addresses that responded with ACK.
    pub fn scan(&mut self) -> Vec<u8> {
        let mut found = Vec::new();
        for addr in SCAN_ADDR_START..=SCAN_ADDR_END {
            // Probe with a zero-length read; ACK means device is present
            let mut buf = [0u8; 1];
            if self.driver.read(addr, &mut buf, SCAN_TIMEOUT_TICKS).is_ok() {
                found.push(addr);
            }
        }
        info!(
            "I2C scan on {}: found {} devices at {:02X?}",
            self.name,
            found.len(),
            found
        );
        self.discovered_addrs = found.clone();
        found
    }

    /// Register discovered devices as child Objects in the O-DF tree (FR-010).
    ///
    /// Each discovered address gets an Object named `I2C_0x{addr:02X}` under
    /// the device root, with an `address` InfoItem containing the numeric address.
    pub fn register_discovered(&self, tree: &mut ObjectTree) {
        use crate::odf::PathTargetMut;
        use std::collections::BTreeMap;

        for &addr in &self.discovered_addrs {
            let child_id = format!("I2C_0x{:02X}", addr);

            let mut child = Object::new(&child_id);
            child.type_uri = Some("omi:i2c:device".into());

            let mut addr_item = crate::odf::InfoItem::new(1);
            addr_item.add_value(OmiValue::Number(addr as f64), None);
            child.add_item("address".into(), addr_item);

            // Add child object under the device root
            let mut children = BTreeMap::new();
            children.insert(child_id.clone(), child);
            if let Err(e) = tree.write_tree(&self.device_path, children) {
                warn!("Failed to register I2C device {}: {}", child_id, e);
                continue;
            }

            info!("I2C device registered: {}/{} (addr 0x{:02X})", self.device_path, child_id, addr);
        }
    }

    /// Return the list of discovered I2C addresses.
    pub fn discovered(&self) -> &[u8] {
        &self.discovered_addrs
    }

    /// Check the TX InfoItem for new values and write to the I2C bus.
    ///
    /// TX writes target the first discovered device by default.
    /// The value is decoded according to the current encoding.
    pub fn sync_tx(&mut self, tree: &ObjectTree) {
        let (value, timestamp, encoding) = match tree.resolve(&self.tx_path) {
            Ok(PathTarget::InfoItem(item)) => {
                let enc = tx_encoding_from_meta(item);
                let vals = item.query_values(Some(1), None, None, None);
                match vals.first() {
                    Some(v) => (v.v.clone(), v.t, enc),
                    None => return,
                }
            }
            _ => return,
        };

        let current = (value, timestamp);
        if self.last_tx.as_ref() == Some(&current) {
            return;
        }

        let tx_bytes = match decode_tx_data(&current.0, encoding) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.last_tx = Some(current);
                return;
            }
            Err(e) => {
                warn!("I2C TX decode error on {}: {}", self.tx_path, e);
                self.last_tx = Some(current);
                return;
            }
        };

        // Write to first discovered device, or addr 0x00 if none discovered
        if let Some(&addr) = self.discovered_addrs.first() {
            if let Err(e) = self.driver.write(addr, &tx_bytes, SCAN_TIMEOUT_TICKS) {
                warn!("I2C TX write to 0x{:02X} on {} failed: {}", addr, self.tx_path, e);
            }
        }

        self.last_tx = Some(current);
    }

    /// Poll for RX data from discovered devices and update the RX InfoItem.
    pub fn poll_rx(&mut self, tree: &mut ObjectTree, now: f64) {
        if self.discovered_addrs.is_empty() {
            return;
        }

        let addr = self.discovered_addrs[0];
        let mut buf = [0u8; DEFAULT_RX_LEN];
        match self.driver.read(addr, &mut buf, SCAN_TIMEOUT_TICKS) {
            Ok(()) => {
                // Trim trailing zeros for cleaner output
                let len = buf.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
                if len > 0 {
                    let value = encode_rx_data(&buf[..len], DataEncoding::String);
                    if let Err(e) = tree.write_value(&self.rx_path, value, Some(now)) {
                        warn!("I2C RX write to {} failed: {}", self.rx_path, e);
                    }
                }
            }
            Err(_) => {} // read failed, device may be busy
        }
    }
}
