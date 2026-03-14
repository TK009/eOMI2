//! JSON parser for the lite-json parser.
//!
//! Parses OMI messages from JSON strings without serde_json.
//! This module will be implemented by the operation sub-parser tasks (T05, T06).

use crate::omi::OmiMessage;
use crate::omi::error::ParseError;

/// Parse an OMI message from a JSON string using the lite-json parser.
///
/// This is the lite-json equivalent of `OmiMessage::parse()` from the serde path.
/// It must produce identical results for all valid OMI messages (FR-012).
pub fn parse_omi_message(_input: &str) -> Result<OmiMessage, ParseError> {
    // Stub: will be implemented by T05 (operation sub-parsers) and T04 (envelope parser).
    // For now, return an error so tests that call this can compile and report meaningful failures.
    Err(ParseError::InvalidJson("lite-json parser not yet implemented".into()))
}
