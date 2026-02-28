// Re-exports and feature gates for std/no_std migration.
//
// Core logic (HTML parsing, JS engine interface, data model) lives here
// so it can eventually be built with #![no_std] for bare-metal targets.
// Platform-specific code (Wi-Fi, HTTP server) stays in main.rs.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod html;
pub mod js;
pub mod device;
