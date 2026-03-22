//! WiFi Secure Onboarding Protocol (WSOP) modules.
//!
//! This module contains the onboarding verification display logic.
//! The WS2812 driver itself lives in [`crate::ws2812`] (NFR-007: independent
//! of WSOP) and is consumed here for the "color" display mode.

pub mod display;
