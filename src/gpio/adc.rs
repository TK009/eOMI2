//! ADC channel management for `analog_in` GPIO pins.
//!
//! The ESP-specific [`AdcPin`] wraps the ESP-IDF oneshot `AdcChannelDriver`.
//! The platform-independent [`AdcChannelSet`] manages multiple ADC channels
//! with conflict detection and coordinated sampling, testable on host via
//! the [`AdcSampler`] trait.

#[cfg(feature = "esp")]
use esp_idf_svc::hal::adc::attenuation::DB_11;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::adc::oneshot::config::AdcChannelConfig;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
#[cfg(feature = "esp")]
use esp_idf_svc::hal::gpio::ADCPin;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::peripheral::Peripheral;
#[cfg(feature = "esp")]
use esp_idf_svc::sys::EspError;
#[cfg(feature = "esp")]
use std::rc::Rc;

use std::collections::HashMap;

/// 12-bit ADC maximum raw value.
pub const ADC_MAX_RAW: u16 = 4095;

/// Trait for sampling an ADC channel, enabling mock implementations for testing.
pub trait AdcSampler {
    /// Sample the raw analog value (0–4095 for 12-bit ADC).
    fn sample(&mut self) -> Result<u16, String>;

    /// GPIO pin number for this channel.
    fn pin_num(&self) -> u8;

    /// Display name for this channel.
    fn name(&self) -> &str;
}

/// Wrapper around an ESP-IDF ADC oneshot channel for reading analog values.
///
/// Each `AdcPin` corresponds to one physical GPIO pin configured as `analog_in`.
/// The `AdcDriver` for the corresponding ADC unit is shared via `Rc` across
/// all channels on the same unit.
#[cfg(feature = "esp")]
pub struct AdcPin<'d, P: ADCPin> {
    channel: AdcChannelDriver<'d, P, Rc<AdcDriver<'d, P::Adc>>>,
    name: String,
    pin_num: u8,
}

#[cfg(feature = "esp")]
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

/// Manages a set of ADC channels with conflict detection and coordinated sampling.
///
/// Tracks allocated GPIO pins to prevent conflicts and provides batch sampling
/// across all registered channels.
#[derive(Default)]
pub struct AdcChannelSet {
    channels: Vec<Box<dyn AdcSampler>>,
    allocated_pins: HashMap<u8, String>,
}

impl AdcChannelSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an ADC channel. Returns `Err` if the pin is already allocated.
    pub fn add_channel(&mut self, channel: Box<dyn AdcSampler>) -> Result<(), String> {
        let pin = channel.pin_num();
        let name = channel.name().to_string();
        if let Some(existing) = self.allocated_pins.get(&pin) {
            return Err(format!(
                "ADC pin conflict: GPIO{} already allocated to '{}', cannot assign to '{}'",
                pin, existing, name
            ));
        }
        self.allocated_pins.insert(pin, name);
        self.channels.push(channel);
        Ok(())
    }

    /// Number of registered channels.
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// Whether any channels are registered.
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Check if a specific GPIO pin is already allocated.
    pub fn is_pin_allocated(&self, pin: u8) -> bool {
        self.allocated_pins.contains_key(&pin)
    }

    /// Sample all registered channels, returning `(pin_num, name, result)` for each.
    pub fn sample_all(&mut self) -> Vec<(u8, String, Result<u16, String>)> {
        self.channels
            .iter_mut()
            .map(|ch| {
                let pin = ch.pin_num();
                let name = ch.name().to_string();
                let result = ch.sample();
                (pin, name, result)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc as StdRc;

    /// Mock ADC sampler for host testing.
    struct MockAdcSampler {
        pin: u8,
        name: String,
        /// Sequence of values to return on successive calls to `sample()`.
        /// If exhausted, returns the last value. If empty, returns Err.
        readings: StdRc<RefCell<Vec<Result<u16, String>>>>,
        call_count: usize,
    }

    impl MockAdcSampler {
        fn new(pin: u8, name: &str, readings: Vec<Result<u16, String>>) -> Self {
            Self {
                pin,
                name: name.to_string(),
                readings: StdRc::new(RefCell::new(readings)),
                call_count: 0,
            }
        }

        fn with_constant(pin: u8, name: &str, value: u16) -> Self {
            Self::new(pin, name, vec![Ok(value)])
        }

        fn with_error(pin: u8, name: &str, msg: &str) -> Self {
            Self::new(pin, name, vec![Err(msg.to_string())])
        }
    }

    impl AdcSampler for MockAdcSampler {
        fn sample(&mut self) -> Result<u16, String> {
            let readings = self.readings.borrow();
            let idx = self.call_count.min(readings.len().saturating_sub(1));
            self.call_count += 1;
            if readings.is_empty() {
                return Err("no readings configured".to_string());
            }
            readings[idx].clone()
        }

        fn pin_num(&self) -> u8 {
            self.pin
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    // --- AdcChannelSet: basic construction ---

    #[test]
    fn new_channel_set_is_empty() {
        let set = AdcChannelSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    // --- Channel allocation ---

    #[test]
    fn add_single_channel() {
        let mut set = AdcChannelSet::new();
        let ch = MockAdcSampler::with_constant(34, "Sensor1", 2048);
        assert!(set.add_channel(Box::new(ch)).is_ok());
        assert_eq!(set.len(), 1);
        assert!(!set.is_empty());
        assert!(set.is_pin_allocated(34));
    }

    #[test]
    fn add_multiple_distinct_channels() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "Light", 1000)))
            .unwrap();
        set.add_channel(Box::new(MockAdcSampler::with_constant(35, "Temp", 2000)))
            .unwrap();
        set.add_channel(Box::new(MockAdcSampler::with_constant(36, "Pressure", 3000)))
            .unwrap();
        assert_eq!(set.len(), 3);
        assert!(set.is_pin_allocated(34));
        assert!(set.is_pin_allocated(35));
        assert!(set.is_pin_allocated(36));
    }

    #[test]
    fn unallocated_pin_returns_false() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "S", 0)))
            .unwrap();
        assert!(!set.is_pin_allocated(35));
        assert!(!set.is_pin_allocated(0));
    }

    // --- Conflict detection ---

    #[test]
    fn duplicate_pin_rejected() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "First", 100)))
            .unwrap();
        let err = set
            .add_channel(Box::new(MockAdcSampler::with_constant(34, "Second", 200)))
            .unwrap_err();
        assert!(
            err.contains("GPIO34"),
            "error should mention pin: {}",
            err
        );
        assert!(
            err.contains("First"),
            "error should mention existing owner: {}",
            err
        );
        assert!(
            err.contains("Second"),
            "error should mention new channel: {}",
            err
        );
        // Original channel still works
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn conflict_does_not_corrupt_state() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "A", 100)))
            .unwrap();
        // Attempt duplicate
        let _ = set.add_channel(Box::new(MockAdcSampler::with_constant(34, "B", 200)));
        // Can still add a different pin
        assert!(set
            .add_channel(Box::new(MockAdcSampler::with_constant(35, "C", 300)))
            .is_ok());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn same_name_different_pins_ok() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "Sensor", 100)))
            .unwrap();
        // Same name but different GPIO pin — allowed
        assert!(set
            .add_channel(Box::new(MockAdcSampler::with_constant(35, "Sensor", 200)))
            .is_ok());
        assert_eq!(set.len(), 2);
    }

    // --- Sampling ---

    #[test]
    fn sample_all_returns_values() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "A", 1024)))
            .unwrap();
        set.add_channel(Box::new(MockAdcSampler::with_constant(35, "B", 2048)))
            .unwrap();
        let results = set.sample_all();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 34); // pin_num
        assert_eq!(results[0].1, "A"); // name
        assert_eq!(results[0].2, Ok(1024));
        assert_eq!(results[1].0, 35);
        assert_eq!(results[1].1, "B");
        assert_eq!(results[1].2, Ok(2048));
    }

    #[test]
    fn sample_empty_set() {
        let mut set = AdcChannelSet::new();
        let results = set.sample_all();
        assert!(results.is_empty());
    }

    // --- Sampling edge cases ---

    #[test]
    fn sample_zero_value() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "S", 0)))
            .unwrap();
        let results = set.sample_all();
        assert_eq!(results[0].2, Ok(0));
    }

    #[test]
    fn sample_max_raw_value() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "S", ADC_MAX_RAW)))
            .unwrap();
        let results = set.sample_all();
        assert_eq!(results[0].2, Ok(ADC_MAX_RAW));
    }

    #[test]
    fn sample_error_propagated() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_error(34, "Bad", "hardware fault")))
            .unwrap();
        let results = set.sample_all();
        assert!(results[0].2.is_err());
        assert!(results[0].2.as_ref().unwrap_err().contains("hardware fault"));
    }

    #[test]
    fn sample_mixed_ok_and_error() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::with_constant(34, "Good", 2048)))
            .unwrap();
        set.add_channel(Box::new(MockAdcSampler::with_error(35, "Bad", "timeout")))
            .unwrap();
        set.add_channel(Box::new(MockAdcSampler::with_constant(36, "Also Good", 1000)))
            .unwrap();
        let results = set.sample_all();
        assert_eq!(results[0].2, Ok(2048));
        assert!(results[1].2.is_err());
        assert_eq!(results[2].2, Ok(1000));
    }

    #[test]
    fn sample_sequence_returns_successive_values() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::new(
            34,
            "Ramp",
            vec![Ok(0), Ok(1024), Ok(2048), Ok(4095)],
        )))
        .unwrap();

        // Each sample_all call advances the sequence
        assert_eq!(set.sample_all()[0].2, Ok(0));
        assert_eq!(set.sample_all()[0].2, Ok(1024));
        assert_eq!(set.sample_all()[0].2, Ok(2048));
        assert_eq!(set.sample_all()[0].2, Ok(4095));
        // After exhaustion, repeats last value
        assert_eq!(set.sample_all()[0].2, Ok(4095));
    }

    #[test]
    fn sample_error_then_recovery() {
        let mut set = AdcChannelSet::new();
        set.add_channel(Box::new(MockAdcSampler::new(
            34,
            "Flaky",
            vec![Err("transient".to_string()), Ok(2048)],
        )))
        .unwrap();

        let r1 = set.sample_all();
        assert!(r1[0].2.is_err());
        let r2 = set.sample_all();
        assert_eq!(r2[0].2, Ok(2048));
    }

    // --- AdcSampler trait on mock ---

    #[test]
    fn mock_sampler_pin_and_name() {
        let sampler = MockAdcSampler::with_constant(34, "TestSensor", 512);
        assert_eq!(sampler.pin_num(), 34);
        assert_eq!(sampler.name(), "TestSensor");
    }

    #[test]
    fn mock_sampler_no_readings_returns_error() {
        let mut sampler = MockAdcSampler::new(34, "Empty", vec![]);
        assert!(sampler.sample().is_err());
    }

    // --- ADC_MAX_RAW constant ---

    #[test]
    fn adc_max_raw_is_12bit() {
        assert_eq!(ADC_MAX_RAW, 0x0FFF);
    }
}
