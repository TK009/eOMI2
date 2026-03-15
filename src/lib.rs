// Re-exports and feature gates for std/no_std migration.
//
// Core logic (data model, OMI engine) lives here so it can eventually be
// built with #![no_std] for bare-metal targets.
// Platform-specific code (Wi-Fi, HTTP server) stays in main.rs / server.rs.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod board;
#[cfg(feature = "esp")]
pub mod boards;
pub mod device;
#[cfg(feature = "std")]
pub mod gpio;
#[cfg(feature = "std")]
pub mod odf;
#[cfg(all(feature = "std", feature = "lite-json"))]
pub mod omi;
#[cfg(feature = "std")]
pub mod pages;
pub mod compress;
pub mod crypto;
pub mod psram;
#[cfg(all(feature = "std", feature = "lite-json"))]
pub mod http;
#[cfg(all(feature = "std", feature = "lite-json"))]
pub mod captive_portal;
#[cfg(feature = "scripting")]
pub mod scripting;
#[cfg(feature = "std")]
pub mod sync_util;
#[cfg(feature = "std")]
pub mod log_util;
#[cfg(feature = "esp")]
pub mod nvs;
#[cfg(feature = "lite-json")]
pub mod json;
#[cfg(feature = "lite-json")]
pub mod wifi_cfg;
pub mod mem_stats;
pub mod wifi_sm;
pub mod callback_url;
pub mod callback;
#[cfg(feature = "esp")]
pub mod temp_sensor;
pub mod time_sync;
#[cfg(feature = "esp")]
pub mod server;
#[cfg(feature = "std")]
pub mod wifi_ap;
#[cfg(feature = "std")]
pub mod dns;
#[cfg(feature = "std")]
pub mod mdns;
#[cfg(feature = "std")]
pub mod mdns_discovery;
