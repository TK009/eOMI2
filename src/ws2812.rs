//! WS2812/NeoPixel RGB LED driver using the ESP-IDF `led_strip` component.
//!
//! Wraps the `espressif/led_strip` managed component with a safe Rust API.
//! Uses the RMT peripheral for precise timing required by WS2812 protocol.
//!
//! This module is **independent of WSOP** (NFR-007) — it can be used by any
//! application code that needs to control a NeoPixel LED.
//!
//! # Usage
//!
//! ```ignore
//! let mut led = Ws2812::new(18)?;   // GPIO 18
//! led.set_color(255, 0, 0)?;        // Red
//! led.off()?;                        // Turn off
//! // led is dropped → resources released
//! ```

use crate::error::{Error, Result};
use esp_idf_svc::sys::*;
use log::info;

/// A single WS2812/NeoPixel RGB LED connected to a GPIO pin via RMT.
pub struct Ws2812 {
    handle: led_strip_handle_t,
    pin: u8,
}

impl Ws2812 {
    /// Create a new WS2812 driver on the given GPIO pin.
    ///
    /// Configures the RMT peripheral with a 10 MHz resolution clock
    /// (100 ns per tick) which is the standard for WS2812 timing.
    pub fn new(pin: u8) -> Result<Self> {
        let rmt_config = led_strip_rmt_config_t {
            clk_src: soc_periph_rmt_clk_src_t_RMT_CLK_SRC_DEFAULT,
            resolution_hz: 10_000_000, // 10 MHz → 100 ns per tick
            flags: led_strip_rmt_config_t__bindgen_ty_1 {
                with_dma: 0,
            },
            mem_block_symbols: 0, // use default
        };

        let strip_config = led_strip_config_t {
            strip_gpio_num: pin as i32,
            max_leds: 1,
            led_model: led_model_t_LED_MODEL_WS2812,
            color_component_format: led_strip_color_component_format_t {
                format: led_color_component_format_t {
                    num_components: 3,
                    __bindgen_anon_1: led_color_component_format_t__bindgen_ty_1 {
                        g_r_b: led_color_component_format_t__bindgen_ty_1__bindgen_ty_1 {
                            _bitfield_1:
                                led_color_component_format_t__bindgen_ty_1__bindgen_ty_1::new_bitfield_1(
                                    0, 1, 2,
                                ),
                            ..Default::default()
                        },
                    },
                },
            },
            flags: led_strip_config_t__bindgen_ty_1 {
                invert_out: 0,
            },
        };

        let mut handle: led_strip_handle_t = core::ptr::null_mut();

        // SAFETY: FFI call to create the LED strip driver. Pointers are valid
        // for the duration of this call. handle is written by the function.
        let err = unsafe {
            led_strip_new_rmt_device(&strip_config, &rmt_config, &mut handle)
        };

        if err != ESP_OK {
            return Err(Error::Owned(format!(
                "led_strip_new_rmt_device failed on GPIO {}: {}",
                pin, err
            )));
        }

        if handle.is_null() {
            return Err(Error::Msg("led_strip_new_rmt_device returned null handle"));
        }

        // Start with LED off
        unsafe {
            led_strip_clear(handle);
        }

        info!("WS2812: initialized on GPIO {}", pin);
        Ok(Self { handle, pin })
    }

    /// Set the LED color. Values are 0–255 for each channel.
    pub fn set_color(&mut self, r: u8, g: u8, b: u8) -> Result<()> {
        // SAFETY: handle is valid (checked in new), pixel index 0 for single LED.
        let err = unsafe { led_strip_set_pixel(self.handle, 0, r as u32, g as u32, b as u32) };
        if err != ESP_OK {
            return Err(Error::Owned(format!(
                "led_strip_set_pixel failed: {}", err
            )));
        }

        let err = unsafe { led_strip_refresh(self.handle) };
        if err != ESP_OK {
            return Err(Error::Owned(format!(
                "led_strip_refresh failed: {}", err
            )));
        }

        Ok(())
    }

    /// Turn the LED off (all channels zero).
    pub fn off(&mut self) -> Result<()> {
        // SAFETY: handle is valid.
        let err = unsafe { led_strip_clear(self.handle) };
        if err != ESP_OK {
            return Err(Error::Owned(format!(
                "led_strip_clear failed: {}", err
            )));
        }
        Ok(())
    }

    /// The GPIO pin this driver is attached to.
    pub fn pin(&self) -> u8 {
        self.pin
    }
}

impl Drop for Ws2812 {
    fn drop(&mut self) {
        // SAFETY: handle is valid. Releases RMT resources.
        unsafe {
            led_strip_del(self.handle);
        }
        info!("WS2812: released GPIO {}", self.pin);
    }
}
