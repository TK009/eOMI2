//! GPIO pin configuration types and O-DF tree builders.
//!
//! Platform-independent types live here. ESP-specific drivers are in submodules
//! gated on the `esp` feature.

#[cfg(feature = "esp")]
pub mod adc;
pub mod encoding;
pub mod peripheral;
pub mod pwm;

use crate::odf::{InfoItem, OmiValue};
use std::collections::BTreeMap;

/// GPIO pin mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpioMode {
    DigitalIn,
    DigitalOut,
    AnalogIn,
    Pwm,
    LowEdgeTrigger,
    HighEdgeTrigger,
}

impl GpioMode {
    /// String representation matching the spec metadata values.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DigitalIn => "digital_in",
            Self::DigitalOut => "digital_out",
            Self::AnalogIn => "analog_in",
            Self::Pwm => "pwm",
            Self::LowEdgeTrigger => "low_edge_trigger",
            Self::HighEdgeTrigger => "high_edge_trigger",
        }
    }

    /// Whether external writes to this pin's InfoItem are allowed (FR-004, FR-006).
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::DigitalOut | Self::Pwm)
    }
}

/// Build-time configuration for a single GPIO pin.
#[derive(Debug, Clone)]
pub struct GpioPinConfig {
    pub pin: u8,
    pub mode: GpioMode,
    pub name: String,
}

impl GpioPinConfig {
    pub fn new(pin: u8, mode: GpioMode) -> Self {
        Self {
            pin,
            mode,
            name: format!("GPIO{}", pin),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

/// Ring buffer capacity for GPIO InfoItems.
const GPIO_CAPACITY: usize = 20;

/// Create an InfoItem for a GPIO pin with appropriate metadata (FR-003).
///
/// Sets `mode` and `gpio_pin` in metadata. Output modes (`digital_out`, `pwm`)
/// are marked writable; input modes are read-only (FR-004, FR-006).
pub fn build_gpio_info_item(config: &GpioPinConfig) -> InfoItem {
    let mut item = InfoItem::new(GPIO_CAPACITY);
    let mut meta = BTreeMap::new();
    meta.insert("mode".into(), OmiValue::Str(config.mode.as_str().into()));
    meta.insert("gpio_pin".into(), OmiValue::Number(config.pin as f64));
    if config.mode.is_writable() {
        meta.insert("writable".into(), OmiValue::Bool(true));
    }
    item.meta = Some(meta);
    item
}

/// Build InfoItems for all configured GPIO pins (FR-002).
///
/// Returns `(name, InfoItem)` pairs to be added to the device root object.
pub fn build_gpio_items(configs: &[GpioPinConfig]) -> Vec<(String, InfoItem)> {
    configs
        .iter()
        .map(|c| (c.name.clone(), build_gpio_info_item(c)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- GpioMode ---

    #[test]
    fn mode_as_str() {
        assert_eq!(GpioMode::DigitalIn.as_str(), "digital_in");
        assert_eq!(GpioMode::DigitalOut.as_str(), "digital_out");
        assert_eq!(GpioMode::AnalogIn.as_str(), "analog_in");
        assert_eq!(GpioMode::Pwm.as_str(), "pwm");
        assert_eq!(GpioMode::LowEdgeTrigger.as_str(), "low_edge_trigger");
        assert_eq!(GpioMode::HighEdgeTrigger.as_str(), "high_edge_trigger");
    }

    #[test]
    fn output_modes_writable() {
        assert!(GpioMode::DigitalOut.is_writable());
        assert!(GpioMode::Pwm.is_writable());
    }

    #[test]
    fn input_modes_not_writable() {
        assert!(!GpioMode::DigitalIn.is_writable());
        assert!(!GpioMode::AnalogIn.is_writable());
        assert!(!GpioMode::LowEdgeTrigger.is_writable());
        assert!(!GpioMode::HighEdgeTrigger.is_writable());
    }

    // --- GpioPinConfig ---

    #[test]
    fn default_name_from_pin_number() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        assert_eq!(cfg.name, "GPIO34");
        assert_eq!(cfg.pin, 34);
        assert_eq!(cfg.mode, GpioMode::AnalogIn);
    }

    #[test]
    fn custom_name() {
        let cfg = GpioPinConfig::new(2, GpioMode::DigitalOut).with_name("LED");
        assert_eq!(cfg.name, "LED");
        assert_eq!(cfg.pin, 2);
    }

    // --- analog_in tree metadata (FR-003, FR-005, FR-006, FR-007) ---

    #[test]
    fn analog_in_has_mode_metadata() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);
        let meta = item.meta.as_ref().expect("metadata should be set");
        assert_eq!(
            meta.get("mode"),
            Some(&OmiValue::Str("analog_in".into()))
        );
    }

    #[test]
    fn analog_in_has_gpio_pin_metadata() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);
        let meta = item.meta.as_ref().unwrap();
        assert_eq!(meta.get("gpio_pin"), Some(&OmiValue::Number(34.0)));
    }

    #[test]
    fn analog_in_not_writable() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);
        assert!(!item.is_writable());
    }

    #[test]
    fn analog_in_no_writable_meta_key() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);
        let meta = item.meta.as_ref().unwrap();
        assert!(
            meta.get("writable").is_none(),
            "analog_in should not have writable key in metadata"
        );
    }

    #[test]
    fn analog_in_starts_empty() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);
        assert!(item.values.is_empty());
    }

    #[test]
    fn analog_in_accepts_adc_range_values() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let mut item = build_gpio_info_item(&cfg);
        // ADC values are 12-bit: 0..4095
        item.add_value(OmiValue::Number(0.0), Some(100.0));
        item.add_value(OmiValue::Number(2048.0), Some(200.0));
        item.add_value(OmiValue::Number(4095.0), Some(300.0));
        assert_eq!(item.values.len(), 3);

        let newest = item.query_values(Some(1), None, None, None);
        assert_eq!(newest[0].v, OmiValue::Number(4095.0));
    }

    #[test]
    fn analog_in_custom_name() {
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn).with_name("LightSensor");
        let items = build_gpio_items(&[cfg]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "LightSensor");
    }

    // --- digital_out writable for contrast ---

    #[test]
    fn digital_out_is_writable() {
        let cfg = GpioPinConfig::new(2, GpioMode::DigitalOut);
        let item = build_gpio_info_item(&cfg);
        assert!(item.is_writable());
        let meta = item.meta.as_ref().unwrap();
        assert_eq!(meta.get("writable"), Some(&OmiValue::Bool(true)));
    }

    // --- build_gpio_items ---

    #[test]
    fn build_multiple_items() {
        let configs = vec![
            GpioPinConfig::new(34, GpioMode::AnalogIn),
            GpioPinConfig::new(2, GpioMode::DigitalOut).with_name("LED"),
            GpioPinConfig::new(5, GpioMode::DigitalIn),
        ];
        let items = build_gpio_items(&configs);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].0, "GPIO34");
        assert_eq!(items[1].0, "LED");
        assert_eq!(items[2].0, "GPIO5");
    }

    #[test]
    fn build_empty_configs() {
        let items = build_gpio_items(&[]);
        assert!(items.is_empty());
    }

    // --- Integration with ObjectTree ---

    #[test]
    fn analog_in_in_object_tree() {
        use crate::odf::{Object, ObjectTree, PathTarget};

        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let item = build_gpio_info_item(&cfg);

        let mut device = Object::new("Device");
        device.add_item("GPIO34".into(), item);

        let mut tree = ObjectTree::new();
        tree.insert_root(device);

        // Verify the item is discoverable in the tree
        match tree.resolve("/Device/GPIO34") {
            Ok(PathTarget::InfoItem(item)) => {
                let meta = item.meta.as_ref().unwrap();
                assert_eq!(meta.get("mode"), Some(&OmiValue::Str("analog_in".into())));
                assert!(!item.is_writable());
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }

    #[test]
    fn analog_in_value_history_in_tree() {
        use crate::odf::{ObjectTree, PathTarget};

        let mut tree = ObjectTree::new();

        // Simulate device boot: add GPIO item via build_gpio_items
        let cfg = GpioPinConfig::new(34, GpioMode::AnalogIn);
        let items = build_gpio_items(&[cfg]);
        let mut device = crate::odf::Object::new("Device");
        for (name, item) in items {
            device.add_item(name, item);
        }
        tree.insert_root(device);

        // Simulate ADC sampling at tick intervals
        if let Ok(PathTarget::InfoItem(item)) =
            tree.resolve("/Device/GPIO34")
        {
            assert!(item.values.is_empty());
        }

        // Write ADC readings (simulating sample_adc at 5s ticks)
        tree.write_value("/Device/GPIO34", OmiValue::Number(1024.0), Some(5.0))
            .unwrap();
        tree.write_value("/Device/GPIO34", OmiValue::Number(2048.0), Some(10.0))
            .unwrap();
        tree.write_value("/Device/GPIO34", OmiValue::Number(3072.0), Some(15.0))
            .unwrap();

        match tree.resolve("/Device/GPIO34") {
            Ok(PathTarget::InfoItem(item)) => {
                assert_eq!(item.values.len(), 3);
                let newest = item.query_values(Some(1), None, None, None);
                assert_eq!(newest[0].v, OmiValue::Number(3072.0));
                assert_eq!(newest[0].t, Some(15.0));
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }
}
