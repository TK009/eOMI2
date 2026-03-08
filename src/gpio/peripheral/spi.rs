//! SPI peripheral bus driver for ESP32 (FR-008, FR-009).
//!
//! Wraps the ESP-IDF SPI device driver. TX writes trigger a full-duplex
//! SPI transfer; the response is written to the RX InfoItem.

use esp_idf_svc::hal::gpio::{InputPin, OutputPin};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::spi::{self, SpiAnyPins, SpiDeviceDriver, SpiDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::EspError;
use log::{info, warn};

use crate::odf::{ObjectTree, OmiValue, PathTarget};

use super::{decode_tx_data, encode_rx_data, tx_encoding_from_meta, DataEncoding};

/// Default SPI clock frequency in Hz (1 MHz).
pub const DEFAULT_FREQ_HZ: u32 = 1_000_000;

/// Configuration for an SPI peripheral bus.
#[derive(Debug, Clone)]
pub struct SpiConfig {
    pub freq_hz: u32,
    pub name: String,
}

impl SpiConfig {
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

/// An SPI bus managing full-duplex transfers via TX/RX InfoItems.
pub struct SpiBus {
    device: SpiDeviceDriver<'static, SpiDriver<'static>>,
    pub(crate) rx_path: String,
    pub(crate) tx_path: String,
    last_tx: Option<(OmiValue, Option<f64>)>,
}

impl SpiBus {
    /// Create a new SPI bus driver.
    ///
    /// `device_path` is the O-DF device root (e.g. `"/MyDevice"`).
    /// Uses `SpiDeviceDriver::new` with a shared `SpiDriver` bus.
    pub fn new<SPI: SpiAnyPins>(
        device_path: &str,
        config: &SpiConfig,
        spi_bus: impl Peripheral<P = SPI> + 'static,
        sclk: impl Peripheral<P = impl OutputPin> + 'static,
        sdo: impl Peripheral<P = impl OutputPin> + 'static,
        sdi: Option<impl Peripheral<P = impl InputPin> + 'static>,
        cs: Option<impl Peripheral<P = impl OutputPin> + 'static>,
    ) -> Result<Self, EspError> {
        let driver_config = spi::config::DriverConfig::new();
        let spi_driver = SpiDriver::new(spi_bus, sclk, sdo, sdi, &driver_config)?;

        let device_config = spi::config::Config::new()
            .baudrate(Hertz(config.freq_hz));
        let device = SpiDeviceDriver::new(spi_driver, cs, &device_config)?;

        let rx_path = format!("{}/{}_SPI_RX", device_path, config.name);
        let tx_path = format!("{}/{}_SPI_TX", device_path, config.name);

        info!(
            "SPI bus created: {} ({}Hz) RX={} TX={}",
            config.name, config.freq_hz, rx_path, tx_path
        );

        Ok(Self {
            device,
            rx_path,
            tx_path,
            last_tx: None,
        })
    }

    /// Check for new TX values and perform a full-duplex SPI transfer.
    ///
    /// When a new value is written to the TX InfoItem, the bytes are
    /// clocked out on MOSI while simultaneously reading from MISO.
    /// The received bytes are written to the RX InfoItem.
    pub fn poll(&mut self, tree: &mut ObjectTree, now: f64) {
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
            return; // already processed this value
        }

        let tx_bytes = match decode_tx_data(&current.0, encoding) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.last_tx = Some(current);
                return;
            }
            Err(e) => {
                warn!("SPI TX decode error on {}: {}", self.tx_path, e);
                self.last_tx = Some(current);
                return;
            }
        };

        // Full-duplex transfer: clock out tx_bytes, read response into rx_buf
        let mut rx_buf = vec![0u8; tx_bytes.len()];
        match self.device.transfer(&mut rx_buf, &tx_bytes) {
            Ok(()) => {
                let rx_value = encode_rx_data(&rx_buf, encoding);
                if let Err(e) = tree.write_value(&self.rx_path, rx_value, Some(now)) {
                    warn!("SPI RX write to {} failed: {}", self.rx_path, e);
                }
            }
            Err(e) => {
                warn!("SPI transfer error on {}: {}", self.tx_path, e);
            }
        }

        self.last_tx = Some(current);
    }
}
