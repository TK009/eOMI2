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

use crate::gpio::{GpioMode, GpioPinConfig};
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
) -> Result<(), anyhow::Error> {
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
            GpioMode::Pwm => {
                warn!(
                    "Board config: PWM pin {} ({}) requires manual LEDC channel/timer setup — skipped",
                    cfg.pin, cfg.name
                );
            }
            GpioMode::AnalogIn => {
                warn!(
                    "Board config: ADC pin {} ({}) requires manual ADC driver setup — skipped",
                    cfg.pin, cfg.name
                );
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
