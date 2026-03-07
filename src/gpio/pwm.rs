// PWM/LEDC driver for ESP32 GPIO pins.
//
// Wraps esp-idf-svc's LedcDriver with 5kHz default frequency and 8-bit
// resolution. Out-of-range duty values are clamped to [0, 255].

#[cfg(feature = "esp")]
use std::collections::BTreeMap;

#[cfg(feature = "esp")]
use esp_idf_svc::hal::adc::{AdcChannelDriver, AdcDriver};
#[cfg(feature = "esp")]
use esp_idf_svc::hal::adc::attenuation::DB_11;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::gpio::{ADCPin, AnyIOPin, Input, InterruptType, OutputPin, PinDriver};
#[cfg(feature = "esp")]
use esp_idf_svc::hal::ledc::{
    config::TimerConfig, LedcChannel, LedcDriver, LedcTimer, LedcTimerDriver, Resolution,
};
#[cfg(feature = "esp")]
use esp_idf_svc::hal::peripheral::Peripheral;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::units::Hertz;
#[cfg(feature = "esp")]
use esp_idf_svc::sys::EspError;
#[cfg(feature = "esp")]
use log::{info, warn};
#[cfg(feature = "esp")]
use std::cell::RefCell;
#[cfg(feature = "esp")]
use std::rc::Rc;

#[cfg(feature = "esp")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "esp")]
use std::sync::Arc;

#[cfg(feature = "esp")]
use crate::gpio::driver::{DigitalInputPin, DigitalOutputPin};
#[cfg(feature = "esp")]
use crate::gpio::driver::parse_digital;
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

/// Edge type for interrupt-driven GPIO pins.
#[cfg(feature = "esp")]
#[derive(Debug, Clone, Copy)]
pub enum EdgeType {
    /// Falling edge (high → low).
    Low,
    /// Rising edge (low → high).
    High,
}

/// A single edge-triggered input pin with an ISR-driven flag.
#[cfg(feature = "esp")]
struct EdgePin {
    path: String,
    pin_num: u8,
    edge_type: EdgeType,
    fired: Arc<AtomicBool>,
    _driver: PinDriver<'static, AnyIOPin, Input>,
}

/// An edge event drained from the ISR flags.
#[cfg(feature = "esp")]
pub struct EdgeEvent {
    pub path: String,
    pub pin_num: u8,
    pub edge_type: EdgeType,
}

/// A type-erased ADC channel entry.
///
/// Uses a boxed closure to sample the ADC, hiding the generic pin type.
/// The closure captures both the shared `AdcDriver` (via `Rc<RefCell>`)
/// and the pin's `AdcChannelDriver`.
#[cfg(feature = "esp")]
struct AdcEntry {
    path: String,
    pin_num: u8,
    sampler: Box<dyn FnMut() -> Result<u16, EspError>>,
}

/// Manages GPIO pins including PWM outputs, edge-triggered inputs, and ADC inputs.
///
/// Provides [`write_pwm`](Self::write_pwm) for direct duty control,
/// [`sync_from_tree`](Self::sync_from_tree) for polling-based actuation,
/// [`drain_edge_events`](Self::drain_edge_events) for interrupt-driven
/// edge detection (FR-005a), and [`sample_adc`](Self::sample_adc) for
/// periodic analog reads (FR-005).
#[cfg(feature = "esp")]
pub struct GpioManager {
    pwm_pins: Vec<PwmPin>,
    edge_pins: Vec<EdgePin>,
    adc_pins: Vec<AdcEntry>,
    digital_inputs: Vec<DigitalInputPin>,
    digital_outputs: Vec<DigitalOutputPin>,
}

#[cfg(feature = "esp")]
impl GpioManager {
    pub fn new() -> Self {
        Self {
            pwm_pins: Vec::new(),
            edge_pins: Vec::new(),
            adc_pins: Vec::new(),
            digital_inputs: Vec::new(),
            digital_outputs: Vec::new(),
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

    /// Register GPIO InfoItems in the O-DF tree (PWM, edge triggers, ADC).
    ///
    /// PWM pins get `mode=pwm` metadata and `writable=true`, initial value 0.
    /// Edge trigger pins get `mode=low_edge_trigger`/`high_edge_trigger`
    /// metadata, read-only. ADC pins get `mode=analog_in` metadata, read-only.
    /// Called once after tree initialisation (FR-003, FR-006).
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

        for pin in &self.edge_pins {
            let mode_str = match pin.edge_type {
                EdgeType::Low => "low_edge_trigger",
                EdgeType::High => "high_edge_trigger",
            };
            // Initialize with no value; first edge event will populate it
            if let Err(e) = tree.write_value(&pin.path, OmiValue::Number(0.0), None) {
                warn!("Failed to init edge InfoItem at {}: {}", pin.path, e);
                continue;
            }
            if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(&pin.path) {
                item.type_uri = Some("omi:gpio:edge".into());
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("mode".into(), OmiValue::Str(mode_str.into()));
                meta.insert("gpio_pin".into(), OmiValue::Number(pin.pin_num as f64));
            }
            info!("Edge InfoItem registered at {} (read-only, mode={})", pin.path, mode_str);
        }

        for entry in &self.adc_pins {
            if let Err(e) = tree.write_value(&entry.path, OmiValue::Number(0.0), None) {
                warn!("Failed to init ADC InfoItem at {}: {}", entry.path, e);
                continue;
            }
            if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(&entry.path) {
                item.type_uri = Some("omi:gpio:analog_in".into());
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("mode".into(), OmiValue::Str("analog_in".into()));
                meta.insert("gpio_pin".into(), OmiValue::Number(entry.pin_num as f64));
            }
            info!("ADC InfoItem registered at {} (read-only, mode=analog_in)", entry.path);
        }

        for pin in &self.digital_inputs {
            if let Err(e) = tree.write_value(&pin.path, OmiValue::Number(0.0), None) {
                warn!("Failed to init digital input InfoItem at {}: {}", pin.path, e);
                continue;
            }
            if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(&pin.path) {
                item.type_uri = Some("omi:gpio:digital_in".into());
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("mode".into(), OmiValue::Str("digital_in".into()));
                meta.insert("gpio_pin".into(), OmiValue::Number(pin.pin_num as f64));
            }
            info!("Digital input InfoItem registered at {} (read-only, mode=digital_in)", pin.path);
        }

        for pin in &self.digital_outputs {
            if let Err(e) = tree.write_value(&pin.path, OmiValue::Number(0.0), None) {
                warn!("Failed to init digital output InfoItem at {}: {}", pin.path, e);
                continue;
            }
            if let Ok(PathTargetMut::InfoItem(item)) = tree.resolve_mut(&pin.path) {
                item.type_uri = Some("omi:gpio:digital_out".into());
                let meta = item.meta.get_or_insert_with(BTreeMap::new);
                meta.insert("mode".into(), OmiValue::Str("digital_out".into()));
                meta.insert("writable".into(), OmiValue::Bool(true));
                meta.insert("gpio_pin".into(), OmiValue::Number(pin.pin_num as f64));
            }
            info!("Digital output InfoItem registered at {} (writable, mode=digital_out)", pin.path);
        }
    }

    /// Returns true if any PWM pins are registered.
    pub fn has_pwm_pins(&self) -> bool {
        !self.pwm_pins.is_empty()
    }

    /// Register an edge-triggered input pin with ISR subscription (FR-005a).
    ///
    /// The ISR sets an `AtomicBool` flag when the configured edge transition
    /// occurs. Call [`drain_edge_events`](Self::drain_edge_events) from the
    /// main loop to collect fired events.
    pub fn add_edge_pin(
        &mut self,
        path: String,
        pin_num: u8,
        pin: AnyIOPin,
        edge_type: EdgeType,
    ) -> Result<(), anyhow::Error> {
        let interrupt_type = match edge_type {
            EdgeType::Low => InterruptType::NegEdge,
            EdgeType::High => InterruptType::PosEdge,
        };

        let mut driver = PinDriver::input(pin)?;
        driver.set_interrupt_type(interrupt_type)?;

        let fired = Arc::new(AtomicBool::new(false));
        let fired_isr = fired.clone();

        unsafe {
            driver.subscribe(move || {
                fired_isr.store(true, Ordering::Release);
            })?;
        }
        driver.enable_interrupt()?;

        let mode_str = match edge_type {
            EdgeType::Low => "low_edge_trigger",
            EdgeType::High => "high_edge_trigger",
        };
        info!("Edge pin registered: {} (GPIO{}, {})", path, pin_num, mode_str);

        self.edge_pins.push(EdgePin {
            path,
            pin_num,
            edge_type,
            fired,
            _driver: driver,
        });
        Ok(())
    }

    /// Drain all fired edge events since the last call (FR-005a).
    ///
    /// Atomically swaps each pin's flag and returns events for pins that
    /// fired. Re-enables the interrupt for each drained pin so the next
    /// edge is captured.
    pub fn drain_edge_events(&mut self) -> Vec<EdgeEvent> {
        let mut events = Vec::new();
        for pin in &mut self.edge_pins {
            if pin.fired.swap(false, Ordering::Acquire) {
                events.push(EdgeEvent {
                    path: pin.path.clone(),
                    pin_num: pin.pin_num,
                    edge_type: pin.edge_type,
                });
                // Re-enable interrupt after servicing (ESP-IDF disables on trigger)
                if let Err(e) = pin._driver.enable_interrupt() {
                    warn!("Failed to re-enable interrupt for {}: {}", pin.path, e);
                }
            }
        }
        events
    }

    /// Returns true if any edge-triggered pins are registered.
    pub fn has_edge_pins(&self) -> bool {
        !self.edge_pins.is_empty()
    }

    /// Register an ADC input pin for periodic sampling.
    ///
    /// The `adc_driver` is shared (via `Rc<RefCell>`) across all channels on
    /// the same ADC unit. Create one `AdcDriver` per unit and pass a clone
    /// of the `Rc` for each pin.
    pub fn add_adc<P: ADCPin + 'static>(
        &mut self,
        path: String,
        pin_num: u8,
        pin: impl Peripheral<P = P> + 'static,
        adc_driver: Rc<RefCell<AdcDriver<'static, P::Adc>>>,
    ) -> Result<(), anyhow::Error> {
        let mut channel = AdcChannelDriver::<{ DB_11 }, P>::new(pin)?;
        info!("ADC pin registered: {} (GPIO{})", path, pin_num);
        self.adc_pins.push(AdcEntry {
            path,
            pin_num,
            sampler: Box::new(move || {
                let mut drv = adc_driver.borrow_mut();
                drv.read(&mut channel)
            }),
        });
        Ok(())
    }

    /// Sample all registered ADC pins and write values to the O-DF tree.
    ///
    /// Each reading is stored as `OmiValue::Number(0..4095)` with a timestamp.
    /// Called from the main loop at the tick interval (FR-005, FR-007).
    pub fn sample_adc(&mut self, tree: &mut ObjectTree, now: f64) {
        for entry in &mut self.adc_pins {
            match (entry.sampler)() {
                Ok(val) => {
                    if let Err(e) = tree.write_value(
                        &entry.path,
                        OmiValue::Number(val as f64),
                        Some(now),
                    ) {
                        warn!("ADC write failed for {}: {}", entry.path, e);
                    }
                }
                Err(e) => warn!("ADC sample failed for {}: {}", entry.path, e),
            }
        }
    }

    /// Returns true if any ADC pins are registered.
    pub fn has_adc_pins(&self) -> bool {
        !self.adc_pins.is_empty()
    }

    /// Register a digital input pin for polling (FR-005).
    pub fn add_digital_in(
        &mut self,
        path: String,
        pin_num: u8,
        pin: AnyIOPin,
    ) -> Result<(), anyhow::Error> {
        let input = DigitalInputPin::new(path.clone(), pin_num, pin)?;
        info!("Digital input registered: {} (GPIO{})", path, pin_num);
        self.digital_inputs.push(input);
        Ok(())
    }

    /// Register a digital output pin (FR-004).
    pub fn add_digital_out(
        &mut self,
        path: String,
        pin_num: u8,
        pin: AnyIOPin,
    ) -> Result<(), anyhow::Error> {
        let output = DigitalOutputPin::new(path.clone(), pin_num, pin)?;
        info!("Digital output registered: {} (GPIO{})", path, pin_num);
        self.digital_outputs.push(output);
        Ok(())
    }

    /// Poll all digital input pins and write changed values to the O-DF tree.
    ///
    /// Only writes when a pin's level has changed since the last poll,
    /// keeping tree writes minimal. Called from the main loop at 100ms
    /// cadence (FR-005, FR-007).
    pub fn poll_digital_inputs(&mut self, tree: &mut ObjectTree, now: f64) {
        for pin in &mut self.digital_inputs {
            if let Some(level) = pin.poll() {
                let value = if level { 1.0 } else { 0.0 };
                if let Err(e) = tree.write_value(
                    &pin.path,
                    OmiValue::Number(value),
                    Some(now),
                ) {
                    warn!("Digital input write failed for {}: {}", pin.path, e);
                }
            }
        }
    }

    /// Synchronize digital outputs from the O-DF tree (FR-004).
    ///
    /// Reads the latest value for each registered digital output path.
    /// If the value changed since the last actuation, the physical pin
    /// is updated. Called from the main loop after engine writes.
    pub fn sync_digital_outputs(&mut self, tree: &ObjectTree) {
        for pin in &mut self.digital_outputs {
            let high = match tree.resolve(&pin.path) {
                Ok(PathTarget::InfoItem(item)) => {
                    let vals = item.query_values(Some(1), None, None, None);
                    match vals.first() {
                        Some(v) => parse_digital(&v.v),
                        None => continue,
                    }
                }
                _ => continue,
            };
            if let Err(e) = pin.write(high) {
                warn!("Digital output write failed for {}: {}", pin.path, e);
            }
        }
    }

    /// Set a digital output pin by O-DF path.
    pub fn write_digital(&mut self, path: &str, high: bool) -> Result<(), String> {
        let pin = self
            .digital_outputs
            .iter_mut()
            .find(|p| p.path == path)
            .ok_or_else(|| format!("no digital output at path '{}'", path))?;
        pin.write(high)
            .map(|_| ())
            .map_err(|e| format!("digital write failed: {}", e))
    }

    /// Returns true if any digital input or output pins are registered.
    pub fn has_digital_pins(&self) -> bool {
        !self.digital_inputs.is_empty() || !self.digital_outputs.is_empty()
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
