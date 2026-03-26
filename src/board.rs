//! Board configuration integration (FR-001, FR-013).
//!
//! When a board TOML file is selected via `EOMI_BOARD=<name>` and the `gpio`
//! feature is enabled, `build.rs` generates `gpio_config.rs` with const arrays
//! and sets the `has_board_config` cfg flag. This module includes those
//! generated constants and provides helpers to convert them into runtime
//! [`GpioPinConfig`] and [`PeripheralConfig`] types.
//!
//! On ESP targets, [`init_gpio`] and [`init_peripherals`] use the board config
//! to register pins with the [`GpioManager`] and [`PeripheralManager`].

use crate::gpio::GpioPinConfig;
#[cfg(any(has_board_config, test, feature = "esp"))]
use crate::gpio::GpioMode;
use crate::gpio::peripheral::{PeripheralConfig, PeripheralProtocol};

#[cfg(has_board_config)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/gpio_config.rs"));
}

/// Board name from TOML config, or "unknown" if no board config was loaded.
pub fn board_name() -> &'static str {
    #[cfg(has_board_config)]
    { generated::BOARD_NAME }
    #[cfg(not(has_board_config))]
    { "unknown" }
}

/// Board chip from TOML config, or "unknown" if no board config was loaded.
pub fn board_chip() -> &'static str {
    #[cfg(has_board_config)]
    { generated::BOARD_CHIP }
    #[cfg(not(has_board_config))]
    { "unknown" }
}

/// Returns true if a board TOML config was loaded at build time.
pub fn has_board_config() -> bool {
    cfg!(has_board_config)
}

/// Whether the board has a temperature sensor, from TOML config.
/// Returns false if no board config was loaded.
pub fn has_temp_sensor() -> bool {
    #[cfg(has_board_config)]
    { generated::HAS_TEMP_SENSOR }
    #[cfg(not(has_board_config))]
    { false }
}

/// GPIO pin connected to an onboard WS2812/NeoPixel LED, if any.
/// Returns None if no board config was loaded or the board has no neopixel.
pub fn neopixel_pin() -> Option<u8> {
    #[cfg(has_board_config)]
    { generated::NEOPIXEL_PIN }
    #[cfg(not(has_board_config))]
    { None }
}

/// Onboarding display mode from board TOML config.
///
/// Returns "color" (WS2812 RGB), "digit" (blink LED), or "none".
/// Returns "none" if no board config was loaded.
pub fn onboard_display_mode() -> &'static str {
    #[cfg(has_board_config)]
    { generated::ONBOARD_DISPLAY_MODE }
    #[cfg(not(has_board_config))]
    { "none" }
}

/// GPIO pin for the board's default LED (named "LED" in the board TOML).
///
/// Used by WSOP digit-mode verification display. Returns `None` if no
/// board config is loaded or no GPIO is named "LED".
pub fn led_pin() -> Option<u8> {
    #[cfg(has_board_config)]
    {
        generated::GPIO_CONFIGS
            .iter()
            .find(|(_, _, name)| name.eq_ignore_ascii_case("led"))
            .map(|(pin, _, _)| *pin)
    }
    #[cfg(not(has_board_config))]
    { None }
}

#[cfg(any(has_board_config, test))]
fn parse_mode(s: &str) -> Option<GpioMode> {
    match s {
        "digital_in" => Some(GpioMode::DigitalIn),
        "digital_out" => Some(GpioMode::DigitalOut),
        "analog_in" => Some(GpioMode::AnalogIn),
        "pwm" => Some(GpioMode::Pwm),
        "low_edge_trigger" => Some(GpioMode::LowEdgeTrigger),
        "high_edge_trigger" => Some(GpioMode::HighEdgeTrigger),
        _ => None,
    }
}

#[cfg(any(has_board_config, test))]
fn parse_protocol(s: &str) -> Option<PeripheralProtocol> {
    match s {
        "I2C" => Some(PeripheralProtocol::I2C),
        "UART" => Some(PeripheralProtocol::UART),
        "SPI" => Some(PeripheralProtocol::SPI),
        _ => None,
    }
}

/// Parse the generated GPIO_CONFIGS into runtime [`GpioPinConfig`] values.
pub fn gpio_configs() -> Vec<GpioPinConfig> {
    #[cfg(has_board_config)]
    {
        generated::GPIO_CONFIGS
            .iter()
            .filter_map(|&(pin, mode_str, name)| {
                let mode = parse_mode(mode_str)?;
                Some(GpioPinConfig::new(pin, mode).with_name(name))
            })
            .collect()
    }
    #[cfg(not(has_board_config))]
    { Vec::new() }
}

/// Parse the generated PERIPHERAL_CONFIGS into runtime [`PeripheralConfig`] values.
pub fn peripheral_configs() -> Vec<PeripheralConfig> {
    #[cfg(has_board_config)]
    {
        generated::PERIPHERAL_CONFIGS
            .iter()
            .filter_map(|&(protocol_str, _pins)| {
                let protocol = parse_protocol(protocol_str)?;
                // Use protocol name as the peripheral name for O-DF paths
                let name = protocol_str;
                Some(PeripheralConfig::new(name, protocol))
            })
            .collect()
    }
    #[cfg(not(has_board_config))]
    { Vec::new() }
}

/// Peripheral config with pin assignments, for board-specific HAL init.
///
/// Extends [`PeripheralConfig`] with the pin-role pairs from the board TOML
/// (e.g., `[(21, "sda"), (22, "scl")]` for I2C). Used by the [`crate::boards`]
/// module to create typed HAL bus drivers.
pub struct PeripheralPinConfig {
    pub protocol: PeripheralProtocol,
    pub name: String,
    pub pins: Vec<(u8, String)>,
}

impl PeripheralPinConfig {
    /// Look up the pin number for a given role (e.g., "sda", "rx").
    pub fn pin_for_role(&self, role: &str) -> Option<u8> {
        self.pins
            .iter()
            .find(|(_, r)| r == role)
            .map(|(pin, _)| *pin)
    }
}

/// Parse the generated PERIPHERAL_CONFIGS into configs with pin assignments.
///
/// Returns the full pin-role mapping from the board TOML, needed by
/// board-specific HAL init modules to allocate typed I2C/UART/SPI drivers.
pub fn peripheral_pin_configs() -> Vec<PeripheralPinConfig> {
    #[cfg(has_board_config)]
    {
        generated::PERIPHERAL_CONFIGS
            .iter()
            .filter_map(|&(protocol_str, pins)| {
                let protocol = parse_protocol(protocol_str)?;
                let pin_pairs: Vec<(u8, String)> = pins
                    .iter()
                    .map(|&(pin, role)| (pin, role.to_string()))
                    .collect();
                Some(PeripheralPinConfig {
                    protocol,
                    name: protocol_str.to_string(),
                    pins: pin_pairs,
                })
            })
            .collect()
    }
    #[cfg(not(has_board_config))]
    { Vec::new() }
}

/// Initialize GPIO pins on the [`GpioManager`] from the board config (ESP only).
///
/// Registers digital_in, digital_out, and edge trigger pins using type-erased
/// `AnyIOPin`. PWM and ADC modes are logged but skipped here because they
/// require typed HAL resources (LEDC channels/timers, ADC drivers) that must
/// be allocated manually per-board.
#[cfg(feature = "esp")]
pub fn init_gpio(
    gpio_manager: &mut crate::gpio::pwm::GpioManager,
    hostname: &str,
) -> crate::error::Result<()> {
    use crate::gpio::pwm::EdgeType;
    use esp_idf_svc::hal::gpio::AnyIOPin;
    use log::{info, warn};

    let configs = gpio_configs();
    if configs.is_empty() {
        return Ok(());
    }

    info!("Board config: initializing {} GPIO pins from {}", configs.len(), board_name());

    for cfg in &configs {
        let path = format!("/{}/{}", hostname, cfg.name);
        match cfg.mode {
            GpioMode::DigitalIn => {
                // SAFETY: pin number comes from build-time validated board TOML
                let pin = unsafe { AnyIOPin::new(cfg.pin as i32) };
                gpio_manager.add_digital_in(path, cfg.pin, pin)?;
            }
            GpioMode::DigitalOut => {
                let pin = unsafe { AnyIOPin::new(cfg.pin as i32) };
                gpio_manager.add_digital_out(path, cfg.pin, pin)?;
            }
            GpioMode::LowEdgeTrigger => {
                let pin = unsafe { AnyIOPin::new(cfg.pin as i32) };
                gpio_manager.add_edge_pin(path, cfg.pin, pin, EdgeType::Low)?;
            }
            GpioMode::HighEdgeTrigger => {
                let pin = unsafe { AnyIOPin::new(cfg.pin as i32) };
                gpio_manager.add_edge_pin(path, cfg.pin, pin, EdgeType::High)?;
            }
            GpioMode::Pwm | GpioMode::AnalogIn => {
                // Handled by board-specific typed HAL init in crate::boards.
                // These modes require typed LEDC/ADC resources from Peripherals
                // that cannot be allocated with AnyIOPin.
            }
        }
    }

    Ok(())
}

/// Initialize peripheral buses on the [`PeripheralManager`] from the board config (ESP only).
///
/// Logs the configured peripherals. Actual driver initialization (UART, SPI,
/// I2C) requires typed HAL peripheral resources and must be done in main.rs
/// using the pin assignments from [`peripheral_configs`].
#[cfg(feature = "esp")]
pub fn log_peripheral_config() {
    use log::info;

    let configs = peripheral_configs();
    if configs.is_empty() {
        return;
    }

    info!("Board config: {} peripheral buses configured", configs.len());

    #[cfg(has_board_config)]
    for &(protocol_str, pins) in generated::PERIPHERAL_CONFIGS.iter() {
        let pin_desc: Vec<String> = pins
            .iter()
            .map(|&(pin, role)| format!("{}={}", role, pin))
            .collect();
        info!("  {}: {}", protocol_str, pin_desc.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_modes() {
        assert_eq!(parse_mode("digital_in"), Some(GpioMode::DigitalIn));
        assert_eq!(parse_mode("digital_out"), Some(GpioMode::DigitalOut));
        assert_eq!(parse_mode("analog_in"), Some(GpioMode::AnalogIn));
        assert_eq!(parse_mode("pwm"), Some(GpioMode::Pwm));
        assert_eq!(parse_mode("low_edge_trigger"), Some(GpioMode::LowEdgeTrigger));
        assert_eq!(parse_mode("high_edge_trigger"), Some(GpioMode::HighEdgeTrigger));
        assert_eq!(parse_mode("invalid"), None);
    }

    #[test]
    fn parse_all_protocols() {
        assert_eq!(parse_protocol("I2C"), Some(PeripheralProtocol::I2C));
        assert_eq!(parse_protocol("UART"), Some(PeripheralProtocol::UART));
        assert_eq!(parse_protocol("SPI"), Some(PeripheralProtocol::SPI));
        assert_eq!(parse_protocol("CAN"), None);
    }

    #[test]
    fn has_board_config_matches_cfg() {
        // In host tests, has_board_config is not set unless EOMI_BOARD is configured
        let expected = cfg!(has_board_config);
        assert_eq!(has_board_config(), expected);
    }

    #[test]
    fn gpio_configs_returns_vec() {
        // Should not panic regardless of whether board config is loaded
        let configs = gpio_configs();
        if has_board_config() {
            assert!(!configs.is_empty(), "board config loaded but gpio_configs is empty");
            for cfg in &configs {
                // Every config should have a valid name
                assert!(!cfg.name.is_empty());
            }
        }
    }

    #[test]
    fn peripheral_configs_returns_vec() {
        let configs = peripheral_configs();
        if has_board_config() {
            // Board may or may not have peripherals, just verify no panic
            for cfg in &configs {
                assert!(!cfg.name.is_empty());
            }
        }
    }

    #[test]
    fn has_temp_sensor_returns_bool() {
        // Should not panic; returns generated value or false
        let val = has_temp_sensor();
        if !has_board_config() {
            assert!(!val, "has_temp_sensor should be false without board config");
        }
    }

    #[test]
    fn onboard_display_mode_returns_value() {
        let mode = onboard_display_mode();
        if has_board_config() {
            // When board config is loaded, mode should be one of the valid values
            assert!(
                ["color", "digit", "none"].contains(&mode),
                "unexpected display mode: {}",
                mode
            );
        } else {
            assert_eq!(mode, "none");
        }
    }

    #[test]
    fn board_name_returns_value() {
        if has_board_config() {
            assert_ne!(board_name(), "unknown");
            assert_ne!(board_chip(), "unknown");
        } else {
            assert_eq!(board_name(), "unknown");
            assert_eq!(board_chip(), "unknown");
        }
    }
}
