//! UART peripheral bus driver for ESP32 (FR-008, FR-009).
//!
//! Wraps the ESP-IDF UART driver. RX data is polled in the 100ms main loop
//! and written to the RX InfoItem. TX data is read from the TX InfoItem
//! and transmitted when new values appear.

use esp_idf_svc::hal::gpio::{InputPin, OutputPin};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::uart::{self, UartDriver};
use esp_idf_svc::sys::EspError;
use log::{info, warn};

use crate::odf::{ObjectTree, OmiValue, PathTarget};

use super::{decode_tx_data, encode_rx_data, tx_encoding_from_meta, DataEncoding};

/// Default UART baud rate.
pub const DEFAULT_BAUD: u32 = 115_200;

/// Maximum bytes to read per poll cycle.
const RX_BUF_SIZE: usize = 256;

/// Non-blocking read timeout (0 ticks).
const NON_BLOCK: u32 = 0;

/// Configuration for a UART peripheral bus.
#[derive(Debug, Clone)]
pub struct UartConfig {
    pub baud: u32,
    pub name: String,
}

impl UartConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            baud: DEFAULT_BAUD,
            name: name.into(),
        }
    }

    pub fn with_baud(mut self, baud: u32) -> Self {
        self.baud = baud;
        self
    }
}

/// A UART bus managing RX polling and TX writes.
pub struct UartBus {
    driver: UartDriver<'static>,
    pub(crate) rx_path: String,
    pub(crate) tx_path: String,
    last_tx: Option<(OmiValue, Option<f64>)>,
}

impl UartBus {
    /// Create a new UART bus driver.
    ///
    /// `device_path` is the O-DF device root (e.g. `"/MyDevice"`).
    /// The RX/TX InfoItem paths are derived as `{device_path}/{name}_UART_RX`
    /// and `{device_path}/{name}_UART_TX`.
    pub fn new<U: uart::Uart>(
        device_path: &str,
        config: &UartConfig,
        uart: impl Peripheral<P = U> + 'static,
        tx_pin: impl Peripheral<P = impl OutputPin> + 'static,
        rx_pin: impl Peripheral<P = impl InputPin> + 'static,
    ) -> Result<Self, EspError> {
        let uart_config = uart::config::Config::new().baudrate(
            esp_idf_svc::hal::units::Hertz(config.baud),
        );
        let driver = UartDriver::new(
            uart,
            tx_pin,
            rx_pin,
            Option::<gpio_stub::AnyIOPin>::None,
            Option::<gpio_stub::AnyIOPin>::None,
            &uart_config,
        )?;

        let rx_path = format!("{}/{}_UART_RX", device_path, config.name);
        let tx_path = format!("{}/{}_UART_TX", device_path, config.name);

        info!(
            "UART bus created: {} ({}baud) RX={} TX={}",
            config.name, config.baud, rx_path, tx_path
        );

        Ok(Self {
            driver,
            rx_path,
            tx_path,
            last_tx: None,
        })
    }

    /// Poll for received UART data and write it to the RX InfoItem.
    pub fn poll_rx(&mut self, tree: &mut ObjectTree, now: f64) {
        let mut buf = [0u8; RX_BUF_SIZE];
        match self.driver.read(&mut buf, NON_BLOCK) {
            Ok(n) if n > 0 => {
                let value = encode_rx_data(&buf[..n], DataEncoding::String);
                if let Err(e) = tree.write_value(&self.rx_path, value, Some(now)) {
                    warn!("UART RX write to {} failed: {}", self.rx_path, e);
                }
            }
            Ok(_) => {} // no data available
            Err(e) => {
                warn!("UART RX read error: {}", e);
            }
        }
    }

    /// Check the TX InfoItem for new values and transmit them.
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
            return; // already transmitted this value
        }

        match decode_tx_data(&current.0, encoding) {
            Ok(bytes) if !bytes.is_empty() => {
                match self.driver.write(&bytes) {
                    Ok(n) => {
                        if n < bytes.len() {
                            warn!(
                                "UART TX partial write on {}: {}/{} bytes",
                                self.tx_path, n, bytes.len()
                            );
                        }
                    }
                    Err(e) => {
                        warn!("UART TX write error on {}: {}", self.tx_path, e);
                    }
                }
            }
            Ok(_) => {} // empty data, nothing to send
            Err(e) => {
                warn!("UART TX decode error on {}: {}", self.tx_path, e);
            }
        }

        self.last_tx = Some(current);
    }
}

/// Workaround module for passing `None` to CTS/RTS pin parameters.
mod gpio_stub {
    pub use esp_idf_svc::hal::gpio::AnyIOPin;
}
