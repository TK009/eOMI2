//! Onboarding verification display (FR-130 through FR-133).
//!
//! Displays a verification code derived from the device's public key so the
//! owner can visually confirm the device identity during WSOP onboarding.
//!
//! Two display modes:
//!
//! - **Color mode** (FR-130): Drives a WS2812 NeoPixel with one of 8 distinct
//!   colors derived from the top 3 bits of the verification byte.
//! - **Digit mode** (FR-131): Blinks the default LED N times where N = code mod 10.
//!
//! The display remains active until [`OnboardDisplay::stop`] is called when
//! onboarding completes (FR-132). After stop, the NeoPixel is released for
//! application use (FR-133).

#[cfg(feature = "esp")]
use crate::ws2812::Ws2812;
#[cfg(feature = "esp")]
use log::info;

/// The 8-color palette for verification display (FR-130).
///
/// Indexed by the top 3 bits of the BLAKE2b verification byte.
/// Colors chosen for maximum visual distinctness on a single RGB LED.
pub const VERIFY_COLORS: [(u8, u8, u8); 8] = [
    (255, 0, 0),     // 000 = Red
    (0, 255, 0),     // 001 = Green
    (0, 0, 255),     // 010 = Blue
    (255, 255, 0),   // 011 = Yellow
    (0, 255, 255),   // 100 = Cyan
    (255, 0, 255),   // 101 = Magenta
    (255, 255, 255), // 110 = White
    (255, 128, 0),   // 111 = Orange
];

/// Human-readable color names matching [`VERIFY_COLORS`] indices.
pub const COLOR_NAMES: [&str; 8] = [
    "Red", "Green", "Blue", "Yellow", "Cyan", "Magenta", "White", "Orange",
];

/// Map a verification byte to a color index (top 3 bits → 0..7).
pub fn color_index(verify_byte: u8) -> usize {
    (verify_byte >> 5) as usize
}

/// Map a verification byte to a digit (mod 10 → 0..9).
pub fn digit_value(verify_byte: u8) -> u8 {
    verify_byte % 10
}

/// Map a verification byte to an (R, G, B) color tuple.
pub fn verification_color(verify_byte: u8) -> (u8, u8, u8) {
    VERIFY_COLORS[color_index(verify_byte)]
}

/// Map a verification byte to a human-readable color name.
pub fn verification_color_name(verify_byte: u8) -> &'static str {
    COLOR_NAMES[color_index(verify_byte)]
}

/// Onboarding display mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayMode {
    /// WS2812 NeoPixel shows one of 8 verification colors (FR-130).
    Color,
    /// Default LED blinks N times for digit N (FR-131).
    Digit,
    /// No display hardware available.
    None,
}

impl DisplayMode {
    /// Parse from the board config string.
    pub fn from_str(s: &str) -> Self {
        match s {
            "color" => Self::Color,
            "digit" => Self::Digit,
            _ => Self::None,
        }
    }

    /// Convert to the board config string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Digit => "digit",
            Self::None => "none",
        }
    }
}

/// Manages the onboarding verification display (FR-132, FR-133).
///
/// Holds the WS2812 driver (in color mode) or LED pin (in digit mode) while
/// the onboarding display is active. When [`stop`](Self::stop) is called,
/// the NeoPixel is turned off and the [`Ws2812`] driver is returned so
/// the application can reuse it (FR-133).
#[cfg(feature = "esp")]
pub struct OnboardDisplay {
    mode: DisplayMode,
    ws2812: Option<Ws2812>,
    led_pin: Option<u8>,
    active: bool,
}

#[cfg(feature = "esp")]
impl OnboardDisplay {
    /// Create a new display in color mode using a WS2812 driver.
    pub fn color(ws2812: Ws2812) -> Self {
        Self {
            mode: DisplayMode::Color,
            ws2812: Some(ws2812),
            led_pin: None,
            active: false,
        }
    }

    /// Create a new display in digit mode using a standard LED GPIO pin.
    pub fn digit(led_pin: u8) -> Self {
        Self {
            mode: DisplayMode::Digit,
            ws2812: None,
            led_pin: Some(led_pin),
            active: false,
        }
    }

    /// Create a no-op display when no hardware is available.
    pub fn none() -> Self {
        Self {
            mode: DisplayMode::None,
            ws2812: None,
            led_pin: None,
            active: false,
        }
    }

    /// Returns the display mode.
    pub fn mode(&self) -> DisplayMode {
        self.mode
    }

    /// Returns true if the display is currently showing a verification code.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Show the verification code on the display (FR-130, FR-131).
    ///
    /// - Color mode: sets the NeoPixel to the verification color.
    /// - Digit mode: records the digit for the blink loop (caller must
    ///   drive the blink timing via [`blink_tick`]).
    pub fn show(&mut self, verify_byte: u8) -> crate::error::Result<()> {
        match self.mode {
            DisplayMode::Color => {
                let (r, g, b) = verification_color(verify_byte);
                let name = verification_color_name(verify_byte);
                if let Some(ref mut ws) = self.ws2812 {
                    ws.set_color(r, g, b)?;
                    info!("WSOP display: showing {} ({},{},{})", name, r, g, b);
                }
            }
            DisplayMode::Digit => {
                let d = digit_value(verify_byte);
                info!("WSOP display: digit mode, will blink {} times", d);
                // Digit blink pattern is driven by blink_tick(); we just log here.
                // The actual LED toggle happens in the main loop via blink_tick().
            }
            DisplayMode::None => {
                info!("WSOP display: no display hardware, skipping");
            }
        }
        self.active = true;
        Ok(())
    }

    /// Stop the verification display (FR-132).
    ///
    /// Turns off the NeoPixel/LED. In color mode, returns the [`Ws2812`]
    /// driver so it can be registered for application use (FR-133).
    pub fn stop(&mut self) -> Option<Ws2812> {
        if !self.active {
            return self.ws2812.take();
        }

        self.active = false;
        match self.mode {
            DisplayMode::Color => {
                if let Some(ref mut ws) = self.ws2812 {
                    let _ = ws.off();
                }
                info!("WSOP display: stopped, NeoPixel released for app use (FR-133)");
                self.ws2812.take()
            }
            DisplayMode::Digit => {
                // Turn off the LED via raw GPIO
                if let Some(pin) = self.led_pin {
                    unsafe {
                        esp_idf_svc::sys::gpio_set_level(pin as i32, 0);
                    }
                }
                info!("WSOP display: stopped, LED off");
                None
            }
            DisplayMode::None => None,
        }
    }

    /// Digit-mode blink tick (FR-131).
    ///
    /// Call this at a regular interval (e.g. 250ms). Returns `true` when
    /// the LED should be ON for this tick. The pattern is: blink N times
    /// (N = digit), then pause for 4 ticks, then repeat.
    ///
    /// `tick` is a monotonic counter incremented each call.
    /// `verify_byte` is the same byte passed to [`show`].
    pub fn blink_tick(&self, tick: u32, verify_byte: u8) -> bool {
        if self.mode != DisplayMode::Digit || !self.active {
            return false;
        }

        let digit = digit_value(verify_byte) as u32;
        if digit == 0 {
            return false; // 0 means no blinks (steady off)
        }

        // Pattern: [ON OFF] * digit + [OFF OFF OFF OFF] (pause)
        // Each blink is 2 ticks (on + off), pause is 4 ticks
        let cycle_len = digit * 2 + 4;
        let phase = tick % cycle_len;

        // During the blink phase, odd ticks are on, even are off
        phase < digit * 2 && phase % 2 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_index_top_3_bits() {
        assert_eq!(color_index(0b000_00000), 0); // Red
        assert_eq!(color_index(0b001_00000), 1); // Green
        assert_eq!(color_index(0b010_00000), 2); // Blue
        assert_eq!(color_index(0b011_00000), 3); // Yellow
        assert_eq!(color_index(0b100_00000), 4); // Cyan
        assert_eq!(color_index(0b101_00000), 5); // Magenta
        assert_eq!(color_index(0b110_00000), 6); // White
        assert_eq!(color_index(0b111_00000), 7); // Orange
    }

    #[test]
    fn color_index_ignores_lower_bits() {
        // All values 0x00..0x1F should map to index 0 (Red)
        for v in 0u8..32 {
            assert_eq!(color_index(v), 0, "byte {} should be Red", v);
        }
        // All values 0xE0..0xFF should map to index 7 (Orange)
        for v in 224u8..=255 {
            assert_eq!(color_index(v), 7, "byte {} should be Orange", v);
        }
    }

    #[test]
    fn digit_value_mod_10() {
        assert_eq!(digit_value(0), 0);
        assert_eq!(digit_value(1), 1);
        assert_eq!(digit_value(9), 9);
        assert_eq!(digit_value(10), 0);
        assert_eq!(digit_value(255), 5); // 255 % 10 = 5
        assert_eq!(digit_value(42), 2);
    }

    #[test]
    fn verification_color_matches_spec() {
        // From the design doc Section 5.2
        assert_eq!(verification_color(0b000_00000), (255, 0, 0));     // Red
        assert_eq!(verification_color(0b001_00000), (0, 255, 0));     // Green
        assert_eq!(verification_color(0b010_00000), (0, 0, 255));     // Blue
        assert_eq!(verification_color(0b011_00000), (255, 255, 0));   // Yellow
        assert_eq!(verification_color(0b100_00000), (0, 255, 255));   // Cyan
        assert_eq!(verification_color(0b101_00000), (255, 0, 255));   // Magenta
        assert_eq!(verification_color(0b110_00000), (255, 255, 255)); // White
        assert_eq!(verification_color(0b111_00000), (255, 128, 0));   // Orange
    }

    #[test]
    fn verification_color_name_matches() {
        assert_eq!(verification_color_name(0b000_00000), "Red");
        assert_eq!(verification_color_name(0b111_11111), "Orange");
        assert_eq!(verification_color_name(0b110_00000), "White");
    }

    #[test]
    fn display_mode_round_trip() {
        for mode in [DisplayMode::Color, DisplayMode::Digit, DisplayMode::None] {
            assert_eq!(DisplayMode::from_str(mode.as_str()), mode);
        }
    }

    #[test]
    fn display_mode_from_unknown_string() {
        assert_eq!(DisplayMode::from_str(""), DisplayMode::None);
        assert_eq!(DisplayMode::from_str("invalid"), DisplayMode::None);
    }
}
