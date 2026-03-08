//! Lightweight JSON parser for OMI-Lite protocol.
//!
//! Enabled by the `lite-json` feature flag. Mutually exclusive with the
//! `json` feature (which pulls in serde_json).

pub mod error;
pub mod lexer;
pub mod parser;
pub mod serializer;
