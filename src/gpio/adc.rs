//! ADC driver for `analog_in` GPIO pins (ESP-only).
//!
//! Provides [`AdcPin`], a wrapper around the ESP-IDF `AdcChannelDriver` that
//! reads 12-bit analog values (0..4095). Uses 11dB attenuation for full-range
//! measurement (0-3.3V on most ESP32 variants).

use esp_idf_svc::hal::adc::attenuation::DB_11;
use esp_idf_svc::hal::adc::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::ADCPin;
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::sys::EspError;

/// Wrapper around an ESP-IDF ADC channel for reading analog values.
///
/// Each `AdcPin` corresponds to one physical GPIO pin configured as `analog_in`.
/// The ADC driver (`AdcDriver`) for the corresponding ADC unit must be passed
/// to [`sample`](AdcPin::sample) at read time, since multiple channels share
/// a single ADC peripheral.
pub struct AdcPin<'d, P: ADCPin> {
    channel: AdcChannelDriver<'d, { DB_11 }, P>,
    name: String,
    pin_num: u8,
}

impl<'d, P: ADCPin> AdcPin<'d, P> {
    /// Create a new ADC pin with 11dB attenuation (full 0-3.3V range).
    pub fn new(
        pin: impl Peripheral<P = P> + 'd,
        pin_num: u8,
        name: String,
    ) -> Result<Self, EspError> {
        let channel = AdcChannelDriver::new(pin)?;
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

    /// Sample the analog value. Returns 0..4095 (12-bit resolution).
    ///
    /// The caller must provide the `AdcDriver` for the ADC unit this pin
    /// belongs to (ADC1 or ADC2).
    pub fn sample(&mut self, adc: &mut AdcDriver<'d, P::Adc>) -> Result<u16, EspError> {
        adc.read(&mut self.channel)
    }
}
