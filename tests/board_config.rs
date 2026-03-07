//! Integration tests for board TOML config parsing and gpio_config.rs generation.
//!
//! These tests verify the build.rs board_config module logic by directly
//! exercising the generated const arrays when EOMI_BOARD is set.

#[cfg(has_board_config)]
mod with_board {
    // Include the generated gpio_config.rs from OUT_DIR
    include!(concat!(env!("OUT_DIR"), "/gpio_config.rs"));

    #[test]
    fn board_metadata_present() {
        assert!(!BOARD_NAME.is_empty());
        assert!(!BOARD_CHIP.is_empty());
    }

    #[test]
    fn gpio_configs_non_empty() {
        assert!(
            !GPIO_CONFIGS.is_empty(),
            "board config should define at least one GPIO pin"
        );
    }

    #[test]
    fn gpio_modes_are_valid() {
        const VALID: &[&str] = &[
            "digital_in",
            "digital_out",
            "analog_in",
            "pwm",
            "low_edge_trigger",
            "high_edge_trigger",
        ];
        for &(pin, mode, _name) in GPIO_CONFIGS {
            assert!(
                VALID.contains(&mode),
                "pin {} has invalid mode '{}'",
                pin,
                mode
            );
        }
    }

    #[test]
    fn no_duplicate_pins() {
        let mut seen = std::collections::HashSet::new();
        for &(pin, _, _) in GPIO_CONFIGS {
            assert!(seen.insert(pin), "duplicate GPIO pin {}", pin);
        }
        for &(_proto, pins) in PERIPHERAL_CONFIGS {
            for &(pin, _role) in pins {
                assert!(seen.insert(pin), "duplicate pin {} in peripherals", pin);
            }
        }
    }

    #[test]
    fn no_duplicate_names() {
        let mut names = std::collections::HashSet::new();
        for &(_pin, _mode, name) in GPIO_CONFIGS {
            assert!(names.insert(name), "duplicate InfoItem name '{}'", name);
        }
    }

    #[test]
    fn peripheral_configs_have_pins() {
        for &(proto, pins) in PERIPHERAL_CONFIGS {
            assert!(
                !pins.is_empty(),
                "peripheral {} has no pin assignments",
                proto
            );
        }
    }
}

/// When no board config is set, the gpio feature still compiles fine.
#[cfg(not(has_board_config))]
#[test]
fn no_board_config_compiles() {
    // This test just verifies the crate compiles without EOMI_BOARD set.
}
