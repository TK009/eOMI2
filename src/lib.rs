// Re-exports and feature gates for std/no_std migration.
//
// Core logic (data model, OMI engine) lives here so it can eventually be
// built with #![no_std] for bare-metal targets.
// Platform-specific code (Wi-Fi, HTTP server) stays in main.rs / server.rs.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod device;
#[cfg(feature = "std")]
pub mod odf;
#[cfg(feature = "std")]
pub mod omi;
#[cfg(feature = "std")]
pub mod pages;
pub mod psram;
#[cfg(feature = "std")]
pub mod http;
#[cfg(feature = "scripting")]
pub mod scripting;
#[cfg(feature = "esp")]
pub mod dht11;
#[cfg(feature = "esp")]
pub mod nvs;
#[cfg(feature = "esp")]
pub mod server;
