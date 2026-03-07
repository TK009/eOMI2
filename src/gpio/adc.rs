//! ADC driver for `analog_in` GPIO pins (ESP-only).
//!
//! Provides [`AdcPin`], a wrapper around the ESP-IDF oneshot `AdcChannelDriver`
//! that reads analog values. Uses 11dB attenuation for full-range measurement
//! (0-3.3V on most ESP32 variants).

use esp_idf_svc::hal::adc::attenuation::DB_11;
use esp_idf_svc::hal::adc::oneshot::config::AdcChannelConfig;
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::ADCPin;
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::sys::EspError;
use std::rc::Rc;

/// Wrapper around an ESP-IDF ADC oneshot channel for reading analog values.
///
/// Each `AdcPin` corresponds to one physical GPIO pin configured as `analog_in`.
/// The `AdcDriver` for the corresponding ADC unit is shared via `Rc` across
/// all channels on the same unit.
pub struct AdcPin<'d, P: ADCPin> {
    channel: AdcChannelDriver<'d, P, Rc<AdcDriver<'d, P::Adc>>>,
    name: String,
    pin_num: u8,
}

impl<'d, P: ADCPin> AdcPin<'d, P> {
    /// Create a new ADC pin with 11dB attenuation (full 0-3.3V range).
    pub fn new(
        adc: Rc<AdcDriver<'d, P::Adc>>,
        pin: impl Peripheral<P = P> + 'd,
        pin_num: u8,
        name: String,
    ) -> Result<Self, EspError> {
        let config = AdcChannelConfig {
            attenuation: DB_11,
            ..Default::default()
        };
        let channel = AdcChannelDriver::new(adc, pin, &config)?;
        Ok(Self {
            channel,
            name,
            pin_num,
        })
    }

    /// Pin number (GPIO number).
    pub fn pin_num(&self) -> u8 {
        self.pin_num
    }

    /// InfoItem name for this pin in the O-DF tree.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sample the raw analog value.
    pub fn sample(&mut self) -> Result<u16, EspError> {
        self.channel.read_raw()
    }
}
