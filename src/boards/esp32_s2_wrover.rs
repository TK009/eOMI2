//! ESP32-S2-WROVER board-specific HAL initialization.
//!
//! Allocates typed HAL resources (LEDC channels for PWM, ADC drivers for
//! analog inputs, I2C/UART bus drivers) from the ESP-IDF Peripherals struct.
//!
//! This module is the ONLY place that knows about the specific hardware
//! resources available on the ESP32-S2. All type-erased pin handling
//! (digital_in, digital_out, edge triggers) is done in [`crate::board`]
//! using `AnyIOPin`.

use std::rc::Rc;

use anyhow::anyhow;
use esp_idf_svc::hal::adc::oneshot::config::AdcChannelConfig;
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::adc::attenuation::DB_11;
use esp_idf_svc::hal::adc::ADC1;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::i2c::I2C0;
use esp_idf_svc::hal::ledc::LEDC;
use esp_idf_svc::hal::uart::UART1;
use log::{info, warn};

use crate::board;
use crate::gpio::peripheral::PeripheralManager;
use crate::gpio::pwm::GpioManager;
use crate::gpio::{GpioMode, GpioPinConfig};

/// Initialize typed HAL resources for the ESP32-S2-WROVER board.
///
/// Takes ownership of LEDC, ADC1, I2C0, and UART1 peripherals.
/// Reads the board TOML config (via [`crate::board`]) to determine
/// which pins need PWM, ADC, or peripheral bus drivers.
pub fn init_typed_hal(
    ledc: LEDC,
    adc1: ADC1,
    i2c0: I2C0,
    uart1: UART1,
    gpio_manager: &mut GpioManager,
    peripheral_manager: &mut PeripheralManager,
    hostname: &str,
) -> anyhow::Result<()> {
    let configs = board::gpio_configs();

    // --- PWM (LEDC) ---
    let pwm_configs: Vec<&GpioPinConfig> = configs
        .iter()
        .filter(|c| c.mode == GpioMode::Pwm)
        .collect();
    init_pwm(ledc, gpio_manager, &pwm_configs, hostname)?;

    // --- ADC (analog input) ---
    let adc_configs: Vec<&GpioPinConfig> = configs
        .iter()
        .filter(|c| c.mode == GpioMode::AnalogIn)
        .collect();
    init_adc(adc1, gpio_manager, &adc_configs, hostname)?;

    // --- Peripheral buses (I2C, UART) ---
    init_peripherals(i2c0, uart1, peripheral_manager, hostname)?;

    Ok(())
}

/// Initialize PWM pins using LEDC channels and timers.
///
/// ESP32-S2 has 8 LEDC channels and 4 timers. Each PWM pin is allocated
/// one channel and one timer sequentially. Up to 4 PWM pins are supported
/// (limited by timers; channels could be shared across timers for more).
fn init_pwm(
    ledc: LEDC,
    gpio_manager: &mut GpioManager,
    pwm_configs: &[&GpioPinConfig],
    hostname: &str,
) -> anyhow::Result<()> {
    if pwm_configs.is_empty() {
        return Ok(());
    }

    if pwm_configs.len() > 4 {
        warn!(
            "Board has {} PWM pins but only 4 LEDC timers; extras will be skipped",
            pwm_configs.len()
        );
    }

    // Destructure LEDC to get individual channels and timers.
    // Unused resources are zero-sized and dropped silently.
    let LEDC {
        channel0,
        channel1,
        channel2,
        channel3,
        timer0,
        timer1,
        timer2,
        timer3,
        ..
    } = ledc;

    // Each PWM pin gets a dedicated channel + timer. Sequential allocation
    // avoids the ownership issue of moving typed resources in a loop.
    macro_rules! init_pwm_slot {
        ($idx:expr, $channel:expr, $timer:expr) => {
            if let Some(cfg) = pwm_configs.get($idx) {
                let path = format!("/{}/{}", hostname, cfg.name);
                let pin = unsafe { AnyIOPin::new(cfg.pin as i32) };
                gpio_manager.add_pwm(path, cfg.pin, $channel, $timer, pin)?;
                info!("PWM: {} (GPIO{}) on LEDC channel {}", cfg.name, cfg.pin, $idx);
            }
        };
    }

    init_pwm_slot!(0, channel0, timer0);
    init_pwm_slot!(1, channel1, timer1);
    init_pwm_slot!(2, channel2, timer2);
    init_pwm_slot!(3, channel3, timer3);

    Ok(())
}

/// Initialize ADC pins using the ADC1 peripheral.
///
/// ESP32-S2 ADC1 covers GPIO1–GPIO10. Pins outside this range are
/// skipped with a warning. A shared `AdcDriver` is created for the
/// ADC1 unit; each pin gets its own `AdcChannelDriver`.
///
/// Pin-number-to-typed-GPIO matching is required because the `ADCPin`
/// trait is implemented per typed GPIO, not on `AnyIOPin`.
fn init_adc(
    adc1: ADC1,
    gpio_manager: &mut GpioManager,
    adc_configs: &[&GpioPinConfig],
    hostname: &str,
) -> anyhow::Result<()> {
    if adc_configs.is_empty() {
        return Ok(());
    }

    let adc_driver = Rc::new(AdcDriver::new(adc1)?);

    for cfg in adc_configs {
        let path = format!("/{}/{}", hostname, cfg.name);
        if let Err(e) = add_adc_pin(gpio_manager, adc_driver.clone(), cfg.pin, path.clone()) {
            warn!("ADC init failed for {} (GPIO{}): {}", cfg.name, cfg.pin, e);
        } else {
            info!("ADC: {} (GPIO{}) on ADC1", cfg.name, cfg.pin);
        }
    }

    Ok(())
}

/// Map a pin number to a typed GPIO and register it as an ADC channel.
///
/// ESP32-S2 ADC1 pins: GPIO1–GPIO10. Each typed GPIO implements `ADCPin`
/// which the `AdcChannelDriver` requires for channel assignment.
fn add_adc_pin(
    gpio_manager: &mut GpioManager,
    adc_driver: Rc<AdcDriver<'static, ADC1>>,
    pin_num: u8,
    path: String,
) -> anyhow::Result<()> {
    use esp_idf_svc::hal::gpio::*;

    // Macro to reduce repetition for each ADC1-capable pin.
    // SAFETY: Pin number comes from build-time validated board TOML.
    // The PinRegistry in GpioManager ensures no duplicate allocation.
    macro_rules! adc_pin {
        ($($num:literal => $gpio:ident),+ $(,)?) => {
            match pin_num {
                $(
                    $num => {
                        let pin = unsafe { $gpio::new() };
                        gpio_manager.add_adc(path, pin_num, pin, adc_driver)?;
                    }
                )+
                _ => {
                    return Err(anyhow!(
                        "GPIO{} is not ADC1-capable on ESP32-S2 (valid: GPIO1-GPIO10)",
                        pin_num
                    ));
                }
            }
        };
    }

    adc_pin!(
        1 => Gpio1, 2 => Gpio2, 3 => Gpio3, 4 => Gpio4, 5 => Gpio5,
        6 => Gpio6, 7 => Gpio7, 8 => Gpio8, 9 => Gpio9, 10 => Gpio10,
    );

    Ok(())
}

/// Initialize peripheral buses (I2C, UART) from the board config.
///
/// Reads peripheral pin configs from the board TOML and creates the
/// appropriate bus drivers using typed HAL peripheral units.
fn init_peripherals(
    i2c0: I2C0,
    uart1: UART1,
    peripheral_manager: &mut PeripheralManager,
    hostname: &str,
) -> anyhow::Result<()> {
    use crate::gpio::peripheral::i2c::{I2cBus, I2cConfig2};
    use crate::gpio::peripheral::uart::{UartBus, UartConfig};

    let periph_configs = board::peripheral_pin_configs();
    if periph_configs.is_empty() {
        return Ok(());
    }

    let mut i2c_used = false;
    let mut uart_used = false;

    for pcfg in &periph_configs {
        let device_path = format!("/{}", hostname);

        match pcfg.protocol {
            crate::gpio::peripheral::PeripheralProtocol::I2C => {
                if i2c_used {
                    warn!("Only one I2C bus supported on ESP32-S2, skipping extra");
                    continue;
                }
                let sda_pin = pcfg.pin_for_role("sda");
                let scl_pin = pcfg.pin_for_role("scl");
                if let (Some(sda), Some(scl)) = (sda_pin, scl_pin) {
                    let sda = unsafe { AnyIOPin::new(sda as i32) };
                    let scl = unsafe { AnyIOPin::new(scl as i32) };
                    let config = I2cConfig2::new(pcfg.name.clone());
                    let bus = I2cBus::new(&device_path, &config, i2c0, sda, scl)?;
                    peripheral_manager.add_i2c(bus);
                    i2c_used = true;
                    info!("I2C bus initialized: {}", pcfg.name);
                } else {
                    warn!("I2C config missing sda/scl pins, skipping");
                }
                // i2c0 is moved, can't use again
                break;
            }
            crate::gpio::peripheral::PeripheralProtocol::UART => {
                if uart_used {
                    warn!("Only one UART bus supported via UART1, skipping extra");
                    continue;
                }
                let rx_pin = pcfg.pin_for_role("rx");
                let tx_pin = pcfg.pin_for_role("tx");
                if let (Some(rx), Some(tx)) = (rx_pin, tx_pin) {
                    let rx = unsafe { AnyIOPin::new(rx as i32) };
                    let tx = unsafe { AnyIOPin::new(tx as i32) };
                    let config = UartConfig::new(pcfg.name.clone());
                    let bus = UartBus::new(&device_path, &config, uart1, tx, rx)?;
                    peripheral_manager.add_uart(bus);
                    uart_used = true;
                    info!("UART bus initialized: {}", pcfg.name);
                } else {
                    warn!("UART config missing rx/tx pins, skipping");
                }
                break;
            }
            crate::gpio::peripheral::PeripheralProtocol::SPI => {
                // SPI requires SPI2/SPI3 peripheral + CS pin.
                // Not yet wired from board TOML; add when needed.
                warn!("SPI peripheral bus init not yet implemented from board config");
            }
        }
    }

    Ok(())
}
