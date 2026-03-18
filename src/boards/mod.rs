//! Board-specific HAL resource allocation.
//!
//! This module separates board-variant-specific typed HAL initialisation
//! from the generic main loop. Each board variant has its own submodule
//! that allocates LEDC channels, ADC drivers, and peripheral bus drivers
//! from the ESP-IDF HAL `Peripherals` struct.
//!
//! Platform-independent config parsing remains in [`crate::board`].
//! This module handles the ESP-specific typed resource allocation that
//! cannot be done generically with `AnyIOPin` (PWM, ADC, peripheral buses).
//!
//! # Adding a new board
//!
//! 1. Create `boards/<board-name>.toml` with GPIO/peripheral config
//! 2. Create `src/boards/<board_name>.rs` with typed HAL init
//! 3. Add a cfg gate below (e.g., `#[cfg(board_chip = "esp32c3")]`)
//! 4. Add the board chip cfg emission in `build.rs`

#[cfg(all(feature = "esp", has_board_config))]
mod esp32_s2_wrover;

#[cfg(feature = "esp")]
use crate::gpio::peripheral::PeripheralManager;
#[cfg(feature = "esp")]
use crate::gpio::pwm::GpioManager;
#[cfg(feature = "esp")]
use esp_idf_svc::hal::prelude::Peripherals;

/// Result of board-specific HAL initialization.
///
/// Contains the configured managers and the modem peripheral
/// (needed by main.rs for WiFi init).
#[cfg(feature = "esp")]
pub struct BoardPeripherals {
    pub modem: esp_idf_svc::hal::modem::Modem,
    pub gpio_manager: GpioManager,
    pub peripheral_manager: PeripheralManager,
}

/// Initialize board GPIO and peripheral managers from HAL peripherals.
///
/// Consumes the `Peripherals` struct, creates [`GpioManager`] and
/// [`PeripheralManager`], populates them from the board TOML config
/// (digital/edge pins via [`crate::board::init_gpio`]), then delegates
/// to the board-specific module for typed HAL resources (PWM, ADC,
/// I2C, UART, SPI).
///
/// Returns [`BoardPeripherals`] containing the configured managers and
/// the modem peripheral for WiFi setup.
#[cfg(feature = "esp")]
pub fn init_board(
    peripherals: Peripherals,
    hostname: &str,
) -> crate::error::Result<BoardPeripherals> {
    use log::info;

    // Suppress onboard WS2812/NeoPixel LED FIRST, before any other GPIO
    // init.  The WS2812 on GPIO 18 (Saola-1) interprets even brief output
    // glitches as pixel data, so we must drive it low before anything else
    // touches the GPIO subsystem.
    //
    // We use raw ESP-IDF calls to set the output register LOW *before*
    // enabling the output direction, avoiding the glitch that PinDriver::
    // output() causes (it enables the driver before set_low, giving the
    // WS2812 time to latch a white pixel).
    #[cfg(has_board_config)]
    if let Some(pin_num) = crate::board::neopixel_pin() {
        use esp_idf_svc::sys::{gpio_set_level, gpio_set_direction, gpio_mode_t_GPIO_MODE_OUTPUT, gpio_reset_pin};
        unsafe {
            gpio_reset_pin(pin_num as i32);
            gpio_set_level(pin_num as i32, 0);
            gpio_set_direction(pin_num as i32, gpio_mode_t_GPIO_MODE_OUTPUT);
        }
        info!("Neopixel: GPIO{} driven low (WS2812 disabled)", pin_num);
    }

    let mut gpio_manager = GpioManager::new();
    let mut peripheral_manager = PeripheralManager::new();

    if crate::board::has_board_config() {
        info!(
            "Board: {} ({})",
            crate::board::board_name(),
            crate::board::board_chip()
        );
        crate::board::init_gpio(&mut gpio_manager, hostname)?;
        crate::board::log_peripheral_config();
    }

    // Board-specific typed HAL init (PWM, ADC, peripheral buses).
    // This block only compiles when a board TOML was loaded at build time.
    //
    // NOTE: Currently dispatches unconditionally to esp32_s2_wrover.
    // When adding a second board, gate on a per-chip cfg flag emitted
    // by build.rs (e.g., `#[cfg(board_chip = "esp32c3")]`).
    #[cfg(has_board_config)]
    esp32_s2_wrover::init_typed_hal(
        peripherals.ledc,
        peripherals.adc1,
        peripherals.i2c0,
        peripherals.uart1,
        &mut gpio_manager,
        &mut peripheral_manager,
        hostname,
    )?;

    Ok(BoardPeripherals {
        modem: peripherals.modem,
        gpio_manager,
        peripheral_manager,
    })
}
