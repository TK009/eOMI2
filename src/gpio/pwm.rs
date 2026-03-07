// PWM/LEDC driver for ESP32 GPIO pins.
//
// Wraps esp-idf-svc's LedcDriver with 5kHz default frequency and 8-bit
// resolution. Out-of-range duty values are clamped to [0, 255].

#[cfg(feature = "esp")]
use std::collections::BTreeMap;

#[cfg(feature = "esp")]
use esp_idf_svc::hal::ledc::{
    config::TimerConfig, LedcChannel, LedcDriver, LedcTimer, LedcTimerDriver, Resolution,
};
#[cfg(feature = "esp")]
use esp_idf_svc::hal::gpio::OutputPin;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::peripheral::Peripheral;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::units::Hertz;
#[cfg(feature = "esp")]
use log::{info, warn};

#[cfg(feature = "esp")]
use crate::odf::{ObjectTree, OmiValue, PathTarget, PathTargetMut};
#[cfg(all(feature = "std", not(feature = "esp")))]
use crate::odf::OmiValue;

/// Default PWM frequency in Hz.
pub const DEFAULT_FREQ_HZ: u32 = 5_000;

/// 8-bit PWM resolution: duty range is [0, 255].
pub const PWM_MAX_DUTY: u32 = 255;

/// Parse an OmiValue as a PWM duty cycle (0–255), clamping out-of-range values.
#[cfg(feature = "std")]
pub fn parse_duty(v: &OmiValue) -> u32 {
    match v {
        OmiValue::Number(n) => {
            let duty = *n as i64;
            duty.clamp(0, PWM_MAX_DUTY as i64) as u32
        }
        OmiValue::Str(s) => s
            .parse::<f64>()
            .ok()
            .map(|n| (n as i64).clamp(0, PWM_MAX_DUTY as i64) as u32)
            .unwrap_or(0),
        _ => 0,
    }
}

/// A single PWM output pin wrapping an ESP-IDF LEDC driver.
#[cfg(feature = "esp")]
pub struct PwmPin {
    path: String,
    driver: LedcDriver<'static>,
    last_duty: u32,
}

#[cfg(feature = "esp")]
impl PwmPin {
    fn set_duty(&mut self, duty: u32) -> Result<(), String> {
        let clamped = duty.min(PWM_MAX_DUTY);
        self.driver
            .set_duty(clamped)
            .map_err(|e| format!("LEDC set_duty failed: {}", e))?;
        self.last_duty = clamped;
        Ok(())
    }
}

/// Manages GPIO pins including PWM outputs.
///
/// Provides [`write_pwm`](Self::write_pwm) for direct duty control and
/// [`sync_from_tree`](Self::sync_from_tree) for polling-based actuation
/// driven by the main loop.
#[cfg(feature = "esp")]
pub struct GpioManager {
    pwm_pins: Vec<PwmPin>,
}

#[cfg(feature = "esp")]
impl GpioManager {
    pub fn new() -> Self {
        Self {
            pwm_pins: Vec::new(),
        }
    }

    /// Create a LEDC timer + channel driver and register a PWM pin.
    ///
    /// Uses [`DEFAULT_FREQ_HZ`] (5 kHz) and 8-bit resolution.
    pub fn add_pwm<C, T, P>(
        &mut self,
        path: String,
        channel: impl Peripheral<P = C> + 'static,
        timer: impl Peripheral<P = T> + 'static,
        pin: impl Peripheral<P = P> + 'static,
    ) -> Result<(), anyhow::Error>
    where
        C: LedcChannel<SpeedMode = <T as LedcTimer>::SpeedMode>,
        T: LedcTimer + 'static,
        P: OutputPin,
    {
        let timer_config = TimerConfig::new()
            .frequency(Hertz(DEFAULT_FREQ_HZ))
            .resolution(Resolution::Bits8);
        let timer_driver = LedcTimerDriver::new(timer, &timer_config)?;
        let driver = LedcDriver::new(channel, timer_driver, pin)?;

        info!("PWM pin registered: {} ({}Hz, 8-bit)", path, DEFAULT_FREQ_HZ);
        self.pwm_pins.push(PwmPin {
            path,
            driver,
            last_duty: 0,
        });
        Ok(())
    }

    /// Set PWM duty cycle by O-DF path. Duty is clamped to [0, 255].
    pub fn write_pwm(&mut self, path: &str, duty: u32) -> Result<(), String> {
        let pin = self
            .pwm_pins
            .iter_mut()
            .find(|p| p.path == path)
            .ok_or_else(|| format!("no PWM pin at path '{}'", path))?;
        pin.set_duty(duty)
    }

    /// Synchronize PWM outputs from the O-DF tree.
    ///
    /// Reads the latest value for each registered PWM path. If the value
    /// changed since the last actuation, the physical pin is updated.
    /// Called from the main loop on each poll cycle.
    pub fn sync_from_tree(&mut self, tree: &ObjectTree) {
        for pin in &mut self.pwm_pins {
            let duty = match tree.resolve(&pin.path) {
                Ok(PathTarget::InfoItem(item)) => {
                    let vals = item.query_values(Some(1), None, None, None);
                    match vals.first() {
                        Some(v) => parse_duty(&v.v),
                        None => continue,
                    }
                }
                _ => continue,
            };
            if duty != pin.last_duty {
                if let Err(e) = pin.set_duty(duty) {
                    warn!("PWM actuation failed for {}: {}", pin.path, e);
                }
            }
        }
    }

    /// Register writable PWM InfoItems in the O-DF tree.
    ///
    /// Creates an InfoItem at each PWM pin's path with `mode=pwm` metadata
    /// and `writable=true`. Initial value is 0 (off). Called once after tree
    /// initialisation.
    pub fn register_tree_items(&self, tree: &mut ObjectTree) {
        for pin in &self.pwm_pins {
            if let Err(e) = tree.write_value(&pin.path, OmiValue::Number(0.0), None) {
                warn!("Failed to init PWM InfoItem at {}: {}", pin.path, e);
                continue;
            }
            if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(&pin.path) {
                item.type_uri = Some("omi:gpio:pwm".into());
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("mode".into(), OmiValue::Str("pwm".into()));
                meta.insert("writable".into(), OmiValue::Bool(true));
                meta.insert("max_duty".into(), OmiValue::Number(PWM_MAX_DUTY as f64));
                meta.insert(
                    "frequency_hz".into(),
                    OmiValue::Number(DEFAULT_FREQ_HZ as f64),
                );
            }
            info!("PWM InfoItem registered at {} (writable, mode=pwm)", pin.path);
        }
    }

    /// Returns true if any PWM pins are registered.
    pub fn has_pwm_pins(&self) -> bool {
        !self.pwm_pins.is_empty()
    }
}

#[cfg(test)]
#[cfg(feature = "std")]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    #[test]
    fn parse_duty_number() {
        assert_eq!(parse_duty(&OmiValue::Number(128.0)), 128);
    }

    #[test]
    fn parse_duty_clamps_high() {
        assert_eq!(parse_duty(&OmiValue::Number(300.0)), PWM_MAX_DUTY);
    }

    #[test]
    fn parse_duty_clamps_negative() {
        assert_eq!(parse_duty(&OmiValue::Number(-10.0)), 0);
    }

    #[test]
    fn parse_duty_zero() {
        assert_eq!(parse_duty(&OmiValue::Number(0.0)), 0);
    }

    #[test]
    fn parse_duty_max() {
        assert_eq!(parse_duty(&OmiValue::Number(255.0)), PWM_MAX_DUTY);
    }

    #[test]
    fn parse_duty_string() {
        assert_eq!(parse_duty(&OmiValue::Str("128".into())), 128);
    }

    #[test]
    fn parse_duty_string_clamps() {
        assert_eq!(parse_duty(&OmiValue::Str("999".into())), PWM_MAX_DUTY);
    }

    #[test]
    fn parse_duty_invalid_string() {
        assert_eq!(parse_duty(&OmiValue::Str("not_a_number".into())), 0);
    }

    #[test]
    fn parse_duty_bool_returns_zero() {
        assert_eq!(parse_duty(&OmiValue::Bool(true)), 0);
    }

    #[test]
    fn parse_duty_null_returns_zero() {
        assert_eq!(parse_duty(&OmiValue::Null), 0);
    }

    #[test]
    fn parse_duty_fractional_truncates() {
        assert_eq!(parse_duty(&OmiValue::Number(127.9)), 127);
    }
}
