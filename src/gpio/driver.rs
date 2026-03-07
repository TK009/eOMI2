//! Digital I/O driver types for ESP GPIO pins.
//!
//! Hardware pin types ([`DigitalInputPin`], [`DigitalOutputPin`]) require the
//! `esp` feature. The [`parse_digital`] helper is available on all `std` targets
//! for host testing.

#[cfg(feature = "esp")]
use esp_idf_svc::hal::gpio::{AnyIOPin, Input, Output, PinDriver};
#[cfg(feature = "esp")]
use esp_idf_svc::sys::EspError;

use crate::odf::OmiValue;

/// A digital input pin with change detection.
///
/// Wraps a `PinDriver<Input>` and tracks the last-read level so that
/// the main loop only writes to the O-DF tree when the value changes.
#[cfg(feature = "esp")]
pub struct DigitalInputPin {
    pub(crate) path: String,
    pub(crate) pin_num: u8,
    pub(crate) driver: PinDriver<'static, AnyIOPin, Input>,
    pub(crate) last_level: Option<bool>,
}

#[cfg(feature = "esp")]
impl DigitalInputPin {
    /// Create a new digital input pin.
    pub fn new(path: String, pin_num: u8, pin: AnyIOPin) -> Result<Self, EspError> {
        let driver = PinDriver::input(pin)?;
        Ok(Self {
            path,
            pin_num,
            driver,
            last_level: None,
        })
    }

    /// Read the current logic level.
    pub fn read(&self) -> bool {
        self.driver.is_high()
    }

    /// Read and return the level only if it changed since the last poll.
    ///
    /// Returns `Some(level)` on the first read or when the level differs
    /// from the previous call. Returns `None` if unchanged.
    pub fn poll(&mut self) -> Option<bool> {
        let level = self.read();
        if self.last_level == Some(level) {
            return None;
        }
        self.last_level = Some(level);
        Some(level)
    }
}

/// A digital output pin with last-written value tracking.
///
/// Wraps a `PinDriver<Output>` and tracks the last level so that
/// redundant writes are skipped.
#[cfg(feature = "esp")]
pub struct DigitalOutputPin {
    pub(crate) path: String,
    pub(crate) pin_num: u8,
    pub(crate) driver: PinDriver<'static, AnyIOPin, Output>,
    pub(crate) last_level: Option<bool>,
}

#[cfg(feature = "esp")]
impl DigitalOutputPin {
    /// Create a new digital output pin (initially low).
    pub fn new(path: String, pin_num: u8, pin: AnyIOPin) -> Result<Self, EspError> {
        let mut driver = PinDriver::output(pin)?;
        driver.set_low()?;
        Ok(Self {
            path,
            pin_num,
            driver,
            last_level: Some(false),
        })
    }

    /// Set the output level. Returns `Ok(true)` if the level actually changed.
    pub fn write(&mut self, high: bool) -> Result<bool, EspError> {
        if self.last_level == Some(high) {
            return Ok(false);
        }
        if high {
            self.driver.set_high()?;
        } else {
            self.driver.set_low()?;
        }
        self.last_level = Some(high);
        Ok(true)
    }
}

/// Parse an OmiValue as a digital boolean (FR-004).
///
/// Truthy: `Number(x)` where `x != 0`, `Bool(true)`, `Str("1"|"true"|"high"|"on")`.
/// Everything else is falsy.
pub fn parse_digital(v: &OmiValue) -> bool {
    match v {
        OmiValue::Number(n) => *n != 0.0,
        OmiValue::Bool(b) => *b,
        OmiValue::Str(s) => matches!(s.as_str(), "1" | "true" | "high" | "on"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_digital_number_nonzero_is_true() {
        assert!(parse_digital(&OmiValue::Number(1.0)));
        assert!(parse_digital(&OmiValue::Number(255.0)));
        assert!(parse_digital(&OmiValue::Number(-1.0)));
    }

    #[test]
    fn parse_digital_number_zero_is_false() {
        assert!(!parse_digital(&OmiValue::Number(0.0)));
    }

    #[test]
    fn parse_digital_bool() {
        assert!(parse_digital(&OmiValue::Bool(true)));
        assert!(!parse_digital(&OmiValue::Bool(false)));
    }

    #[test]
    fn parse_digital_truthy_strings() {
        for s in &["1", "true", "high", "on"] {
            assert!(parse_digital(&OmiValue::Str((*s).into())), "expected truthy: {}", s);
        }
    }

    #[test]
    fn parse_digital_falsy_strings() {
        for s in &["0", "false", "low", "off", ""] {
            assert!(!parse_digital(&OmiValue::Str((*s).into())), "expected falsy: {}", s);
        }
    }

    #[test]
    fn parse_digital_null_is_false() {
        assert!(!parse_digital(&OmiValue::Null));
    }
}
