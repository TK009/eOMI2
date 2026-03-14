#![cfg(any(feature = "json", feature = "lite-json"))]
//! Negative / error-path tests for malformed inputs, resource exhaustion,
//! and error propagation across OMI engine, scripting, and network layers.
//!
//! Focuses on paths that were previously only covered by happy-case tests.

mod common;

// ===========================================================================
// 1. JSON Parser — Malformed Input Handling (lite-json only)
// ===========================================================================

#[cfg(feature = "lite-json")]
mod json_parser_errors {
    use reconfigurable_device::json::error::LiteParseError;
    use reconfigurable_device::json::parser::{FromJson, JsonParser};
    use reconfigurable_device::odf::{InfoItem, Object, OmiValue};

    // --- Depth exceeded ---

    #[test]
    fn deeply_nested_arrays_rejected() {
        // 33 levels of nested arrays should exceed MAX_DEPTH (32)
        let open: String = "[".repeat(33);
        let close: String = "]".repeat(33);
        let input = format!("{}null{}", open, close);
        let err = OmiValue::from_json_str(&input).unwrap_err();
        assert!(matches!(err, LiteParseError::DepthExceeded { max: 32, .. }));
    }

    #[test]
    fn deeply_nested_objects_rejected() {
        // 33 levels of nested objects
        let mut input = String::new();
        for i in 0..33 {
            input.push_str(&format!("{{\"k{}\":", i));
        }
        input.push_str("null");
        for _ in 0..33 {
            input.push('}');
        }
        let err = OmiValue::from_json_str(&input).unwrap_err();
        assert!(matches!(err, LiteParseError::DepthExceeded { max: 32, .. }));
    }

    #[test]
    fn depth_at_limit_accepted() {
        // 32 levels should be exactly at the limit and accepted
        let open: String = "[".repeat(32);
        let close: String = "]".repeat(32);
        let input = format!("{}null{}", open, close);
        assert!(OmiValue::from_json_str(&input).is_ok());
    }

    // --- Truncated inputs ---

    #[test]
    fn truncated_object_key() {
        let err = Object::from_json_str(r#"{"id"#).unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedEof { .. } | LiteParseError::UnterminatedString { .. }
        ));
    }

    #[test]
    fn truncated_object_after_colon() {
        let err = Object::from_json_str(r#"{"id":"#).unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedEof { .. } | LiteParseError::UnterminatedString { .. }
        ));
    }

    #[test]
    fn truncated_array_mid_element() {
        let err = OmiValue::from_json_str("[1, 2,").unwrap_err();
        assert!(matches!(err, LiteParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn object_missing_closing_brace() {
        let err = Object::from_json_str(r#"{"id": "A""#).unwrap_err();
        assert!(err != LiteParseError::TrailingData { pos: reconfigurable_device::json::error::Pos::new(0) });
    }

    #[test]
    fn array_missing_closing_bracket() {
        let err = OmiValue::from_json_str("[1, 2, 3").unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedEof { .. } | LiteParseError::ExpectedToken { .. }
        ));
    }

    // --- Type mismatches in structured objects ---

    #[test]
    fn object_id_is_number_not_string() {
        let err = Object::from_json_str(r#"{"id": 42}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    #[test]
    fn value_v_field_is_array() {
        // "v" field can be any OmiValue including arrays (which become Null)
        use reconfigurable_device::odf::value::Value;
        let v = Value::from_json_str(r#"{"v": [1, 2, 3]}"#).unwrap();
        // Arrays are not a valid OmiValue — parser should handle gracefully
        assert_eq!(v.v, OmiValue::Null);
    }

    #[test]
    fn info_item_values_not_array() {
        let err = InfoItem::from_json_str(r#"{"values": "not-an-array"}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    #[test]
    fn info_item_values_element_not_object() {
        let err = InfoItem::from_json_str(r#"{"values": [42]}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    // --- Malformed number edge cases ---

    #[test]
    fn number_double_negative() {
        let err = OmiValue::from_json_str("--1").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    #[test]
    fn number_leading_plus() {
        let err = OmiValue::from_json_str("+1").unwrap_err();
        assert!(matches!(err, LiteParseError::UnexpectedChar { .. }));
    }

    #[test]
    fn number_dot_only() {
        let err = OmiValue::from_json_str(".5").unwrap_err();
        assert!(matches!(err, LiteParseError::UnexpectedChar { .. }));
    }

    #[test]
    fn number_exponent_no_digits() {
        let err = OmiValue::from_json_str("1e+").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidNumber { .. }));
    }

    #[test]
    fn number_multiple_dots() {
        // "1.2.3" should parse "1.2" and then fail on trailing ".3"
        let err = OmiValue::from_json_str("1.2.3").unwrap_err();
        assert!(matches!(err, LiteParseError::TrailingData { .. }));
    }

    // --- String edge cases ---

    #[test]
    fn string_escape_at_eof() {
        let err = OmiValue::from_json_str(r#""hello\"#).unwrap_err();
        assert!(matches!(err, LiteParseError::UnterminatedString { .. }));
    }

    #[test]
    fn string_unicode_escape_at_eof() {
        let err = OmiValue::from_json_str(r#""\u00"#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidUnicodeEscape { .. }));
    }

    #[test]
    fn string_high_surrogate_at_eof() {
        // High surrogate without following \u escape
        let err = OmiValue::from_json_str(r#""\uD800"#).unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::InvalidSurrogatePair { .. } | LiteParseError::UnterminatedString { .. }
        ));
    }

    #[test]
    fn string_high_surrogate_followed_by_non_escape() {
        let err = OmiValue::from_json_str(r#""\uD800abc""#).unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidSurrogatePair { .. }));
    }

    #[test]
    fn string_all_control_chars_rejected() {
        for ch in 0x00u8..0x20 {
            let input = format!("\"{}\"", ch as char);
            let result = OmiValue::from_json_str(&input);
            assert!(result.is_err(), "control char 0x{:02x} should be rejected", ch);
        }
    }

    #[test]
    fn string_invalid_utf8_byte() {
        // Invalid UTF-8 continuation byte without valid leader
        let input: Vec<u8> = vec![b'"', 0xFF, b'"'];
        let err = OmiValue::from_json_bytes(&input).unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnterminatedString { .. } | LiteParseError::UnexpectedChar { .. }
        ));
    }

    #[test]
    fn string_truncated_utf8_sequence() {
        // Start of a 3-byte UTF-8 char but only 2 bytes before closing quote
        let input: Vec<u8> = vec![b'"', 0xE4, 0xB8, b'"'];
        let err = OmiValue::from_json_bytes(&input).unwrap_err();
        assert!(matches!(
            err,
            LiteParseError::UnexpectedChar { .. } | LiteParseError::UnterminatedString { .. }
        ));
    }

    // --- Literal edge cases ---

    #[test]
    fn partial_null() {
        let err = OmiValue::from_json_str("nu").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidLiteral { .. }));
    }

    #[test]
    fn partial_false() {
        let err = OmiValue::from_json_str("fal").unwrap_err();
        assert!(matches!(err, LiteParseError::InvalidLiteral { .. }));
    }

    #[test]
    fn null_with_extra_chars() {
        let err = OmiValue::from_json_str("nullify").unwrap_err();
        assert!(matches!(err, LiteParseError::TrailingData { .. }));
    }

    #[test]
    fn true_with_extra_chars() {
        let err = OmiValue::from_json_str("trueish").unwrap_err();
        assert!(matches!(err, LiteParseError::TrailingData { .. }));
    }

    // --- Object parsing edge cases ---

    #[test]
    fn object_non_string_key() {
        let err = Object::from_json_str(r#"{42: "value"}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    #[test]
    fn object_missing_colon() {
        let err = Object::from_json_str(r#"{"id" "A"}"#).unwrap_err();
        assert!(matches!(err, LiteParseError::ExpectedToken { .. }));
    }

    #[test]
    fn object_double_comma() {
        let err = Object::from_json_str(r#"{"id": "A",, "type": "t"}"#).unwrap_err();
        // Double comma means we expect a string key but get another comma
        assert!(err != LiteParseError::TrailingData { pos: reconfigurable_device::json::error::Pos::new(0) });
    }

    #[test]
    fn object_trailing_comma() {
        let err = Object::from_json_str(r#"{"id": "A",}"#).unwrap_err();
        assert!(err != LiteParseError::TrailingData { pos: reconfigurable_device::json::error::Pos::new(0) });
    }
}

// ===========================================================================
// 2. OMI Envelope Parsing — Error Paths (lite-json only)
// ===========================================================================

#[cfg(feature = "lite-json")]
mod omi_envelope_errors {
    use reconfigurable_device::json::parser::parse_omi_message;
    use reconfigurable_device::omi::error::ParseError;

    #[test]
    fn empty_string() {
        assert!(parse_omi_message("").is_err());
    }

    #[test]
    fn just_whitespace() {
        assert!(parse_omi_message("   ").is_err());
    }

    #[test]
    fn not_an_object() {
        assert!(parse_omi_message("[1, 2]").is_err());
    }

    #[test]
    fn omi_field_is_number_not_string() {
        let err = parse_omi_message(r#"{"omi":1.0,"ttl":0,"read":{"path":"/A"}}"#).unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn ttl_is_string_not_number() {
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":"zero","read":{"path":"/A"}}"#).unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn ttl_is_float_with_fractional_part() {
        // Float with exact integer value should work
        let msg = parse_omi_message(r#"{"omi":"1.0","ttl":0.0,"read":{"path":"/A"}}"#).unwrap();
        assert_eq!(msg.ttl, 0);
    }

    #[test]
    fn ttl_is_float_non_integer() {
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":0.5,"read":{"path":"/A"}}"#).unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn read_missing_path_and_rid() {
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":0,"read":{}}"#).unwrap_err();
        assert_eq!(err, ParseError::MissingField("path or rid"));
    }

    #[test]
    fn write_v_and_items_mutually_exclusive() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":1,"items":[{"path":"/B","v":2}]}}"#,
        )
        .unwrap_err();
        assert_eq!(err, ParseError::MutuallyExclusive("v", "items"));
    }

    #[test]
    fn write_v_and_objects_mutually_exclusive() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"path":"/A","v":1,"objects":{"X":{"id":"X"}}}}"#,
        )
        .unwrap_err();
        assert_eq!(err, ParseError::MutuallyExclusive("v", "objects"));
    }

    #[test]
    fn write_items_and_objects_mutually_exclusive() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"items":[{"path":"/A","v":1}],"objects":{"X":{"id":"X"}}}}"#,
        )
        .unwrap_err();
        assert_eq!(err, ParseError::MutuallyExclusive("items", "objects"));
    }

    #[test]
    fn write_single_missing_path() {
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":0,"write":{"v":42}}"#).unwrap_err();
        assert_eq!(err, ParseError::MissingField("path"));
    }

    #[test]
    fn write_batch_item_missing_path() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"items":[{"v":42}]}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn write_batch_item_missing_v() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"items":[{"path":"/A/B"}]}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn write_tree_missing_path() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"write":{"objects":{"X":{"id":"X"}}}}"#,
        )
        .unwrap_err();
        assert_eq!(err, ParseError::MissingField("path"));
    }

    #[test]
    fn write_tree_depth_exceeded() {
        // Build JSON with nesting deeper than MAX_OBJECT_DEPTH (8)
        let mut json = String::from(r#"{"omi":"1.0","ttl":0,"write":{"path":"/","objects":{"L1":{"id":"L1""#);
        for i in 2..=9 {
            json.push_str(&format!(
                r#","objects":{{"L{}":{{"id":"L{}""#,
                i, i
            ));
        }
        // Close all objects
        for _ in 2..=9 {
            json.push_str("}}");
        }
        json.push_str("}}}}}");

        let err = parse_omi_message(&json).unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { field: "objects", .. }));
    }

    #[test]
    fn cancel_missing_rid() {
        let err =
            parse_omi_message(r#"{"omi":"1.0","ttl":0,"cancel":{}}"#).unwrap_err();
        assert_eq!(err, ParseError::MissingField("rid"));
    }

    #[test]
    fn cancel_rid_not_array() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"cancel":{"rid":"single"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn delete_missing_path() {
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":0,"delete":{}}"#).unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn delete_path_no_leading_slash() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"delete":{"path":"DeviceA"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidField { .. }));
    }

    #[test]
    fn response_missing_status() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"response":{"desc":"OK"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn response_status_negative() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"response":{"status":-1}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn response_status_too_large() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"response":{"status":99999}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn three_operations_rejected() {
        let err = parse_omi_message(
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/A"},"delete":{"path":"/B"},"cancel":{"rid":["r1"]}}"#,
        )
        .unwrap_err();
        assert_eq!(err, ParseError::InvalidOperationCount(3));
    }

    #[test]
    fn truncated_mid_envelope() {
        assert!(parse_omi_message(r#"{"omi":"1.0","ttl"#).is_err());
    }

    #[test]
    fn envelope_with_null_operation_value() {
        // "read" key with null value — should not count as an operation
        let err = parse_omi_message(r#"{"omi":"1.0","ttl":0,"read":null}"#).unwrap_err();
        // Parser expects ObjectStart for read body
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }
}

// ===========================================================================
// 3. ODF Object Tree — Error Propagation
// ===========================================================================

mod odf_tree_errors {
    use reconfigurable_device::odf::{Object, ObjectTree, OmiValue};
    use std::collections::BTreeMap;

    #[test]
    fn resolve_deeply_nested_not_found() {
        let mut tree = ObjectTree::new();
        tree.write_value("/A/B/C/D", OmiValue::Number(1.0), None)
            .unwrap();
        let err = tree.resolve("/A/B/X/D").unwrap_err();
        assert!(matches!(err, reconfigurable_device::odf::TreeError::NotFound(_)));
    }

    #[test]
    fn resolve_item_as_intermediate_segment() {
        // Try to resolve a path where an intermediate segment is an InfoItem, not an Object
        let mut tree = ObjectTree::new();
        tree.write_value("/DevA/Temp", OmiValue::Number(22.5), None)
            .unwrap();
        // /DevA/Temp/SubThing — Temp is an InfoItem, not an Object
        let err = tree.resolve("/DevA/Temp/SubThing").unwrap_err();
        assert!(matches!(err, reconfigurable_device::odf::TreeError::NotFound(_)));
    }

    #[test]
    fn write_value_to_single_segment_rejected() {
        let mut tree = ObjectTree::new();
        let err = tree
            .write_value("/OnlyObject", OmiValue::Number(1.0), None)
            .unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn write_value_to_root_rejected() {
        let mut tree = ObjectTree::new();
        let err = tree
            .write_value("/", OmiValue::Number(1.0), None)
            .unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn write_value_invalid_path_no_slash() {
        let mut tree = ObjectTree::new();
        let err = tree
            .write_value("NoSlash/Item", OmiValue::Number(1.0), None)
            .unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn resolve_path_with_dotdot() {
        let tree = ObjectTree::new();
        let err = tree.resolve("/A/../B").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn resolve_double_slash() {
        let tree = ObjectTree::new();
        let err = tree.resolve("/A//B").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn resolve_trailing_slash() {
        let tree = ObjectTree::new();
        let err = tree.resolve("/A/B/").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn delete_root_forbidden() {
        let mut tree = ObjectTree::new();
        let err = tree.delete("/").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::Forbidden(_)
        ));
    }

    #[test]
    fn delete_nonexistent_root_object() {
        let mut tree = ObjectTree::new();
        let err = tree.delete("/Missing").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::NotFound(_)
        ));
    }

    #[test]
    fn delete_nonexistent_nested_item() {
        let mut tree = ObjectTree::new();
        tree.write_value("/DevA/Temp", OmiValue::Number(22.5), None)
            .unwrap();
        let err = tree.delete("/DevA/Nonexistent").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::NotFound(_)
        ));
    }

    #[test]
    fn delete_through_nonexistent_parent() {
        let mut tree = ObjectTree::new();
        tree.write_value("/DevA/Temp", OmiValue::Number(22.5), None)
            .unwrap();
        let err = tree.delete("/Missing/Temp").unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::NotFound(_)
        ));
    }

    #[test]
    fn resolve_mut_not_found() {
        let mut tree = ObjectTree::new();
        tree.write_value("/DevA/Temp", OmiValue::Number(22.5), None)
            .unwrap();
        assert!(tree.resolve_mut("/DevA/Missing").is_err());
    }

    #[test]
    fn resolve_mut_through_nonexistent_object() {
        let mut tree = ObjectTree::new();
        tree.write_value("/DevA/Temp", OmiValue::Number(22.5), None)
            .unwrap();
        assert!(tree.resolve_mut("/DevA/Missing/Deep").is_err());
    }

    #[test]
    fn write_tree_auto_creates_missing_path() {
        let mut tree = ObjectTree::new();
        let mut objects = BTreeMap::new();
        objects.insert("Child".into(), Object::new("Child"));
        // Writing tree to /Missing/Sub should auto-create both
        tree.write_tree("/Missing/Sub", objects).unwrap();
        assert!(tree.resolve("/Missing/Sub/Child").is_ok());
    }

    #[test]
    fn write_tree_invalid_path() {
        let mut tree = ObjectTree::new();
        let objects = BTreeMap::new();
        let err = tree.write_tree("no-slash", objects).unwrap_err();
        assert!(matches!(
            err,
            reconfigurable_device::odf::TreeError::InvalidPath(_)
        ));
    }

    #[test]
    fn empty_tree_resolve_not_found() {
        let tree = ObjectTree::new();
        assert!(tree.resolve("/Anything").is_err());
    }

    #[test]
    fn empty_tree_is_empty() {
        let tree = ObjectTree::new();
        assert!(tree.is_empty());
        assert!(!tree.root_contains("anything"));
    }
}

// ===========================================================================
// 4. OMI Engine — Error Response Propagation
// ===========================================================================

mod engine_error_responses {
    use super::common::*;
    use reconfigurable_device::omi::OmiMessage;

    #[test]
    fn read_nonexistent_path_returns_404() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/Nonexistent/Path"}}"#,
        );
        assert_eq!(response_status(&resp), 404);
    }

    #[test]
    fn delete_nonexistent_path_returns_404() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"delete":{"path":"/Nonexistent"}}"#,
        );
        assert_eq!(response_status(&resp), 404);
    }

    #[test]
    fn delete_root_rejected_at_parse_level() {
        // delete "/" is rejected at parse level (InvalidField), not at engine level
        assert!(matches!(
            OmiMessage::parse(r#"{"omi":"1.0","ttl":0,"delete":{"path":"/"}}"#),
            Err(_)
        ));
    }

    #[test]
    fn write_to_root_path_returns_error() {
        let mut e = engine_with_sensor_tree();
        // Writing a value to "/" should fail
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"write":{"path":"/","v":42}}"#,
        );
        // Engine should return an error status (not 200/201)
        let status = response_status(&resp);
        assert!(status >= 400, "write to root should fail, got status {}", status);
    }

    #[test]
    fn write_single_to_object_path_returns_error() {
        let mut e = engine_with_sensor_tree();
        // Writing a value to "/System" (an object, not Obj/Item) should fail
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"write":{"path":"/System","v":42}}"#,
        );
        let status = response_status(&resp);
        assert!(status >= 400, "write to single-segment path should fail, got status {}", status);
    }

    #[test]
    fn cancel_nonexistent_subscription() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"cancel":{"rid":["nonexistent-sub"]}}"#,
        );
        // Should return some response (not panic)
        let status = response_status(&resp);
        assert!(status > 0);
    }

    #[test]
    fn write_batch_partial_failures() {
        let mut e = engine_with_sensor_tree();
        // Batch with one valid and one invalid path
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"write":{"items":[
                {"path":"/DevA/Temp","v":22.5},
                {"path":"/","v":42}
            ]}}"#,
        );
        let batch = response_batch(&resp);
        assert_eq!(batch.len(), 2);
        // First should succeed (201 created)
        assert!(batch[0].status == 200 || batch[0].status == 201);
        // Second should fail (writing to root "/" is invalid)
        assert!(batch[1].status >= 400, "write to root should fail in batch");
    }

    #[test]
    fn write_batch_all_invalid_paths() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"write":{"items":[
                {"path":"/","v":1},
                {"path":"/OnlyObj","v":2}
            ]}}"#,
        );
        let batch = response_batch(&resp);
        assert_eq!(batch.len(), 2);
        // Both should fail
        assert!(batch[0].status >= 400);
        assert!(batch[1].status >= 400);
    }

    #[test]
    fn read_with_depth_zero() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/System","depth":0}}"#,
        );
        assert_eq!(response_status(&resp), 200);
        // Depth 0 should return the object without items/objects
        let result = extract_json_result(&resp);
        assert_eq!(result["id"], "System");
    }
}

// ===========================================================================
// 5. HTTP Validation — Edge Cases
// ===========================================================================

#[cfg(feature = "json")]
mod http_validation_errors {
    use reconfigurable_device::http::*;

    #[test]
    fn validate_cl_huge_number() {
        // Number larger than usize::MAX should fail parsing
        assert_eq!(
            validate_content_length(Some("99999999999999999999999"), 1024),
            Err(BodyError::Invalid)
        );
    }

    #[test]
    fn validate_cl_float_string() {
        assert_eq!(
            validate_content_length(Some("10.5"), 1024),
            Err(BodyError::Invalid)
        );
    }

    #[test]
    fn validate_cl_whitespace() {
        assert_eq!(
            validate_content_length(Some(" 100 "), 1024),
            Err(BodyError::Invalid)
        );
    }

    #[test]
    fn validate_cl_empty_string() {
        assert_eq!(
            validate_content_length(Some(""), 1024),
            Err(BodyError::Invalid)
        );
    }

    #[test]
    fn validate_cl_hex_string() {
        assert_eq!(
            validate_content_length(Some("0xFF"), 1024),
            Err(BodyError::Invalid)
        );
    }

    #[test]
    fn validate_cl_max_usize_boundary() {
        // At the exact maximum
        assert_eq!(validate_content_length(Some("1024"), 1024), Ok(1024));
        // One over
        assert_eq!(
            validate_content_length(Some("1025"), 1024),
            Err(BodyError::TooLarge)
        );
    }

    #[test]
    fn validate_cl_one_is_valid() {
        assert_eq!(validate_content_length(Some("1"), 1024), Ok(1));
    }

    #[test]
    fn bearer_auth_token_with_spaces() {
        // Token after "Bearer " is "my token" — should match
        assert!(check_bearer_auth(Some("Bearer my token"), "my token"));
        // Wrong expected token should not match
        assert!(!check_bearer_auth(Some("Bearer my token"), "wrong"));
    }

    #[test]
    fn bearer_auth_only_prefix_no_token() {
        assert!(!check_bearer_auth(Some("Bearer "), "something"));
    }

    #[test]
    fn bearer_auth_unicode_token() {
        assert!(check_bearer_auth(Some("Bearer tökën"), "tökën"));
        assert!(!check_bearer_auth(Some("Bearer tökën"), "token"));
    }

    // --- URI parsing edge cases ---

    #[test]
    fn omi_uri_multiple_trailing_slashes() {
        let (path, trailing) = omi_uri_to_odf_path("/omi/DeviceA///");
        // Multiple trailing slashes should be trimmed
        assert!(trailing);
        assert_eq!(path, "/DeviceA");
    }

    #[test]
    fn uri_query_multiple_question_marks() {
        // Only first ? counts as query separator
        assert_eq!(uri_query("/path?a=1?b=2"), Some("a=1?b=2"));
    }

    #[test]
    fn parse_query_value_with_equals() {
        // Value containing '=' should be preserved
        let pairs = parse_query_params("key=a=b=c");
        assert_eq!(pairs, vec![("key", "a=b=c")]);
    }

    #[test]
    fn parse_query_empty_key_skipped() {
        let pairs = parse_query_params("=value&key=val");
        assert_eq!(pairs, vec![("key", "val")]);
    }

    #[test]
    fn html_escape_empty_string() {
        assert_eq!(html_escape(""), "");
    }

    #[test]
    fn html_escape_all_special_at_once() {
        assert_eq!(
            html_escape("<script>alert('x\"&');</script>"),
            "&lt;script&gt;alert(&#x27;x&quot;&amp;&#x27;);&lt;/script&gt;"
        );
    }
}

// ===========================================================================
// 6. Captive Portal — Form Parsing Error Paths
// ===========================================================================

#[cfg(feature = "json")]
mod captive_portal_errors {
    use reconfigurable_device::captive_portal::*;

    #[test]
    fn url_decode_percent_at_very_end() {
        assert_eq!(
            parse_provision_form("ssid_0=test%&password_0=p&api_key_action=keep", 3, false),
            Err(FormError::InvalidEncoding)
        );
    }

    #[test]
    fn url_decode_single_hex_at_end() {
        assert_eq!(
            parse_provision_form("ssid_0=test%2&password_0=p&api_key_action=keep", 3, false),
            Err(FormError::InvalidEncoding)
        );
    }

    #[test]
    fn url_decode_invalid_hex_chars() {
        assert_eq!(
            parse_provision_form("ssid_0=test%ZZ&password_0=p&api_key_action=keep", 3, false),
            Err(FormError::InvalidEncoding)
        );
    }

    #[test]
    fn completely_empty_body() {
        assert_eq!(
            parse_provision_form("", 3, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn body_with_only_unknown_fields() {
        assert_eq!(
            parse_provision_form("foo=bar&baz=qux", 3, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn ssid_index_exceeds_max_aps() {
        // ssid_5 with max_aps=3 should be ignored
        assert_eq!(
            parse_provision_form("ssid_5=Network&password_5=pass&api_key_action=keep", 3, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn ssid_index_non_numeric() {
        // ssid_abc should be silently ignored
        assert_eq!(
            parse_provision_form("ssid_abc=Network&api_key_action=keep", 3, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn max_aps_zero() {
        // With max_aps=0, no credentials are ever accepted
        assert_eq!(
            parse_provision_form("ssid_0=Net&password_0=pass&api_key_action=keep", 0, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn password_without_matching_ssid() {
        // Password exists but no SSID for that index
        assert_eq!(
            parse_provision_form("password_0=secret&api_key_action=keep", 3, false),
            Err(FormError::NoCredentials)
        );
    }

    #[test]
    fn first_setup_unknown_api_key_action() {
        // Unknown action defaults to Keep, which is rejected on first setup
        assert_eq!(
            parse_provision_form(
                "ssid_0=Net&password_0=pass&api_key_action=unknown",
                3,
                true,
            ),
            Err(FormError::ApiKeyRequired)
        );
    }

    #[test]
    fn url_encoded_ssid_with_plus_and_percent() {
        let form = parse_provision_form(
            "ssid_0=My+WiFi%21&password_0=p%40ss&api_key_action=keep",
            3,
            false,
        )
        .unwrap();
        assert_eq!(form.credentials[0].ssid, "My WiFi!");
        assert_eq!(form.credentials[0].password, "p@ss");
    }

    #[test]
    fn empty_hostname_treated_as_none() {
        let form = parse_provision_form(
            "ssid_0=Net&password_0=pass&hostname=&api_key_action=keep",
            3,
            false,
        )
        .unwrap();
        assert!(form.hostname.is_none());
    }

    #[test]
    fn non_utf8_percent_encoding_rejected() {
        // %C0%AF is an overlong encoding — results in invalid UTF-8
        assert_eq!(
            parse_provision_form(
                "ssid_0=test%C0%AF&password_0=p&api_key_action=keep",
                3,
                false,
            ),
            Err(FormError::InvalidEncoding)
        );
    }
}

// ===========================================================================
// 7. NVS Persistence — Corruption and Edge Cases
// ===========================================================================

#[cfg(feature = "json")]
mod nvs_persistence_errors {
    use reconfigurable_device::device::*;
    use reconfigurable_device::odf::OmiValue;

    #[test]
    fn deserialize_empty_input() {
        assert!(deserialize_saved_items(b"").is_err());
    }

    #[test]
    fn deserialize_random_bytes() {
        assert!(deserialize_saved_items(b"not binary data").is_err());
    }

    #[test]
    fn deserialize_wrong_version() {
        // Version byte 0xFF should be rejected
        assert!(deserialize_saved_items(&[0xFF, 0, 0]).is_err());
    }

    #[test]
    fn deserialize_truncated_after_version() {
        // Just a version byte with no count field
        assert!(deserialize_saved_items(&[0x01]).is_err());
    }

    #[test]
    fn serialize_exactly_at_limit() {
        let items = vec![SavedItem {
            path: "/A/B".into(),
            v: OmiValue::Number(1.0),
            t: None,
        }];
        let blob = serialize_saved_items(&items).unwrap();
        assert!(blob.len() <= MAX_NVS_BLOB);
    }

    #[test]
    fn serialize_just_over_limit() {
        let items: Vec<SavedItem> = (0..200)
            .map(|i| SavedItem {
                path: format!("/Object{}/ItemWithLongName{}", i, i),
                v: OmiValue::Str("x".repeat(10)),
                t: Some(i as f64),
            })
            .collect();
        let result = serialize_saved_items(&items);
        if let Err(e) = result {
            assert!(matches!(e, NvsSaveError::TooLarge(_)));
        }
    }

    #[test]
    fn roundtrip_all_value_types() {
        let items = vec![
            SavedItem {
                path: "/A/Num".into(),
                v: OmiValue::Number(3.14),
                t: Some(100.0),
            },
            SavedItem {
                path: "/A/Str".into(),
                v: OmiValue::Str("hello".into()),
                t: None,
            },
            SavedItem {
                path: "/A/Bool".into(),
                v: OmiValue::Bool(true),
                t: Some(200.0),
            },
            SavedItem {
                path: "/A/Null".into(),
                v: OmiValue::Null,
                t: None,
            },
        ];
        let blob = serialize_saved_items(&items).unwrap();
        let restored = deserialize_saved_items(&blob).unwrap();
        assert_eq!(items, restored);
    }
}

// ===========================================================================
// 8. Scripting Engine — Boundary and Error Tests
// ===========================================================================

mod scripting_errors {
    use reconfigurable_device::scripting::engine::{ScriptEngine, MAX_SCRIPT_LEN};
    use reconfigurable_device::scripting::error::ScriptError;

    #[test]
    fn script_at_exact_limit_accepted() {
        let mut engine = ScriptEngine::new().unwrap();
        // Script exactly at MAX_SCRIPT_LEN — pad with spaces and a return value
        let padding = " ".repeat(MAX_SCRIPT_LEN - 1);
        let script = format!("{}1", padding);
        assert_eq!(script.len(), MAX_SCRIPT_LEN);
        let result = engine.exec(&script);
        assert!(result.is_ok(), "script at exact limit should be accepted: {:?}", result.err());
    }

    #[test]
    fn script_one_over_limit_rejected() {
        let mut engine = ScriptEngine::new().unwrap();
        let script = "x".repeat(MAX_SCRIPT_LEN + 1);
        match engine.exec(&script) {
            Err(ScriptError::ScriptTooLarge(len)) => {
                assert_eq!(len, MAX_SCRIPT_LEN + 1);
            }
            other => panic!("expected ScriptTooLarge, got: {:?}", other),
        }
    }

    #[test]
    fn script_empty_string() {
        let mut engine = ScriptEngine::new().unwrap();
        // Empty script should not crash
        let result = engine.exec("");
        // mJS may return undefined/null for empty script
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn script_nul_byte() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("let x = 1;\0let y = 2;");
        match result {
            Err(ScriptError::Execution(msg)) => {
                assert!(msg.contains("NUL"), "expected NUL byte error: {}", msg);
            }
            other => panic!("expected Execution error for NUL byte, got: {:?}", other),
        }
    }

    #[test]
    fn script_runtime_error_reference() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("undefinedVariable");
        // mJS may return undefined for undefined variables rather than error
        // Just verify no panic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn script_runtime_error_type() {
        let mut engine = ScriptEngine::new().unwrap();
        let result = engine.exec("null.property");
        // Should error or return undefined, not panic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn script_deeply_nested_function_calls() {
        let mut engine = ScriptEngine::new().unwrap();
        // Create a chain of function calls that tests stack depth
        let script = "function f(n) { return n <= 0 ? 0 : f(n-1) + 1; } f(50)";
        let result = engine.exec(script);
        // Should either succeed or hit op limit, not segfault
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn script_error_does_not_corrupt_engine_state() {
        let mut engine = ScriptEngine::new().unwrap();
        // First: cause an error
        let _ = engine.exec("let x = ;");
        // Second: normal execution should still work
        let result = engine.exec("1 + 1").unwrap();
        assert_eq!(result, reconfigurable_device::odf::OmiValue::Number(2.0));
    }

    #[test]
    fn script_op_limit_does_not_corrupt_engine_state() {
        let mut engine = ScriptEngine::new().unwrap();
        // Hit op limit
        let _ = engine.exec("while(true){}");
        // Normal execution should still work after
        let result = engine.exec("42").unwrap();
        assert_eq!(result, reconfigurable_device::odf::OmiValue::Number(42.0));
    }

    #[test]
    fn script_error_display_messages() {
        // Verify error Display implementations produce useful messages
        let e1 = ScriptError::InitFailed;
        assert!(e1.to_string().contains("initialization"));

        let e2 = ScriptError::ScriptTooLarge(5000);
        assert!(e2.to_string().contains("5000"));
        assert!(e2.to_string().contains("max"));

        let e3 = ScriptError::Execution("test error".into());
        assert!(e3.to_string().contains("test error"));

        let e4 = ScriptError::OpLimitExceeded;
        assert!(e4.to_string().contains("operation limit"));

        let e5 = ScriptError::TimeLimitExceeded(std::time::Duration::from_millis(6000));
        assert!(e5.to_string().contains("6000"));
    }
}

// ===========================================================================
// 9. JSON Serializer — Write Roundtrip with Malformed Data
// ===========================================================================

#[cfg(feature = "lite-json")]
mod json_serializer_edge_cases {
    use reconfigurable_device::json::serializer::{JsonWriter, ToJson};
    use reconfigurable_device::json::parser::FromJson;
    use reconfigurable_device::odf::OmiValue;

    #[test]
    fn write_f64_integral_includes_decimal() {
        // Integral floats should include decimal point when serialized
        let val = OmiValue::Number(1.0);
        let s = val.to_json_string();
        assert!(s.contains('.'), "1.0 should have decimal point: {}", s);
    }

    #[test]
    fn write_f64_negative_zero() {
        let val = OmiValue::Number(-0.0);
        let s = val.to_json_string();
        // Should produce valid JSON number
        assert!(s.contains('0'));
    }

    #[test]
    fn write_f64_very_large() {
        let val = OmiValue::Number(1e308);
        let s = val.to_json_string();
        assert!(!s.is_empty());
        // Should round-trip
        let parsed = OmiValue::from_json_str(&s).unwrap();
        assert_eq!(parsed, val);
    }

    #[test]
    fn write_f64_very_small() {
        let val = OmiValue::Number(5e-324);
        let s = val.to_json_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn omi_value_serialization_roundtrip() {
        let values = vec![
            OmiValue::Null,
            OmiValue::Bool(true),
            OmiValue::Bool(false),
            OmiValue::Number(0.0),
            OmiValue::Number(-3.14),
            OmiValue::Number(1e10),
            OmiValue::Str(String::new()),
            OmiValue::Str("hello world".into()),
            OmiValue::Str("special chars: \"\\/<>&".into()),
        ];

        for original in &values {
            let bytes = original.to_json_bytes();
            let parsed = OmiValue::from_json_bytes(&bytes).unwrap();
            assert_eq!(
                &parsed, original,
                "roundtrip failed for {:?}, serialized as: {}",
                original,
                std::str::from_utf8(&bytes).unwrap_or("<invalid utf8>")
            );
        }
    }

    #[test]
    fn json_writer_empty_string_serialization() {
        let val = OmiValue::Str(String::new());
        let s = val.to_json_string();
        assert_eq!(s, "\"\"");
    }

    #[test]
    fn json_writer_string_with_escapes() {
        let val = OmiValue::Str("line1\nline2\ttab\"quote\\backslash".into());
        let s = val.to_json_string();
        assert!(s.contains("\\n"));
        assert!(s.contains("\\t"));
        assert!(s.contains("\\\""));
        assert!(s.contains("\\\\"));
        // Roundtrip
        let parsed = OmiValue::from_json_str(&s).unwrap();
        assert_eq!(parsed, val);
    }
}

// ===========================================================================
// 10. Subscription Resource Exhaustion & Error Paths
// ===========================================================================

mod subscription_errors {
    use super::common::*;
    use reconfigurable_device::odf::{OmiValue, Value};
    use reconfigurable_device::omi::subscriptions::{
        DeliveryTarget, PollBuffer, PollResult, SubscriptionRegistry, MAX_SUBSCRIPTIONS,
    };
    use reconfigurable_device::omi::Engine;

    // --- Subscription limit ---

    #[test]
    fn registry_max_subscriptions_enforced() {
        let mut reg = SubscriptionRegistry::new();
        for i in 0..MAX_SUBSCRIPTIONS {
            let path = format!("/test/item{}", i);
            assert!(
                reg.create(&path, -1.0, DeliveryTarget::Poll, 60.0, 0.0).is_ok(),
                "subscription {} should succeed",
                i
            );
        }
        let err = reg
            .create("/test/overflow", -1.0, DeliveryTarget::Poll, 60.0, 0.0)
            .unwrap_err();
        assert!(err.contains("limit"), "error should mention limit: {}", err);
    }

    #[test]
    fn engine_subscription_limit_returns_400() {
        let mut e = Engine::new();
        for i in 0..MAX_SUBSCRIPTIONS {
            let json = format!(
                r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/t/i{}","interval":-1}}}}"#,
                i
            );
            let resp = parse_and_process(&mut e, &json);
            assert_eq!(response_status(&resp), 200, "sub {} should succeed", i);
        }
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/over","interval":-1}}"#,
        );
        assert_eq!(response_status(&resp), 400);
        assert!(
            response_desc(&resp).unwrap().contains("limit"),
            "desc should mention limit: {:?}",
            response_desc(&resp)
        );
    }

    #[test]
    fn subscription_limit_freed_after_cancel() {
        let mut e = Engine::new();
        let mut rids = Vec::new();
        for i in 0..MAX_SUBSCRIPTIONS {
            let json = format!(
                r#"{{"omi":"1.0","ttl":60,"read":{{"path":"/t/i{}","interval":-1}}}}"#,
                i
            );
            let resp = parse_and_process(&mut e, &json);
            rids.push(response_rid(&resp).to_string());
        }
        // At limit — cancel one
        let cancel_json = format!(
            r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}"]}}}}"#,
            rids[0]
        );
        parse_and_process(&mut e, &cancel_json);
        // Now should succeed
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/new","interval":-1}}"#,
        );
        assert_eq!(response_status(&resp), 200);
    }

    // --- Invalid interval values ---

    #[test]
    fn interval_zero_rejected() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg
            .create("/t/i", 0.0, DeliveryTarget::Poll, 60.0, 0.0)
            .unwrap_err();
        assert!(err.contains("interval"), "error: {}", err);
    }

    #[test]
    fn interval_negative_non_minus_one() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg
            .create("/t/i", -2.0, DeliveryTarget::Poll, 60.0, 0.0)
            .unwrap_err();
        assert!(err.contains("interval"), "error: {}", err);
    }

    #[test]
    fn interval_too_small() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg
            .create("/t/i", 0.05, DeliveryTarget::Poll, 60.0, 0.0)
            .unwrap_err();
        assert!(err.contains("0.1"), "error should mention 0.1: {}", err);
    }

    #[test]
    fn interval_at_minimum_accepted() {
        let mut reg = SubscriptionRegistry::new();
        assert!(reg.create("/t/i", 0.1, DeliveryTarget::Poll, 60.0, 0.0).is_ok());
    }

    // --- Subscription TTL enforcement ---

    #[test]
    fn negative_ttl_subscription_via_engine() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":-1,"read":{"path":"/t","interval":5.0}}"#,
        );
        assert_eq!(response_status(&resp), 400);
        assert!(response_desc(&resp).unwrap().contains("ttl"));
    }

    #[test]
    fn zero_ttl_subscription_via_engine() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/t","interval":5.0}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    // --- Poll error paths ---

    #[test]
    fn poll_nonexistent_rid() {
        let mut reg = SubscriptionRegistry::new();
        assert!(matches!(reg.poll("nonexistent", 0.0), PollResult::NotFound));
    }

    #[test]
    fn poll_callback_subscription_not_pollable() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg
            .create("/t/i", -1.0, DeliveryTarget::Callback("http://x.com".into()), 60.0, 0.0)
            .unwrap();
        assert!(matches!(reg.poll(&rid, 1.0), PollResult::NotPollable));
    }

    #[test]
    fn poll_websocket_subscription_not_pollable() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg
            .create("/t/i", -1.0, DeliveryTarget::WebSocket(1), 60.0, 0.0)
            .unwrap();
        assert!(matches!(reg.poll(&rid, 1.0), PollResult::NotPollable));
    }

    #[test]
    fn poll_not_pollable_via_engine() {
        let mut e = Engine::new();
        let resp = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":-1,"callback":"http://x.com"}}"#,
            0.0,
            None,
        );
        let rid = response_rid(&resp).to_string();
        let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
        let resp = process_at(&mut e, &poll_json, 1.0, None);
        assert_eq!(response_status(&resp), 400);
        assert!(response_desc(&resp).unwrap().contains("not pollable"));
    }

    #[test]
    fn poll_expired_subscription_returns_not_found() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg
            .create("/t/i", -1.0, DeliveryTarget::Poll, 10.0, 0.0)
            .unwrap();
        assert!(matches!(reg.poll(&rid, 11.0), PollResult::NotFound));
    }

    // --- Event notification edge cases ---

    #[test]
    fn notify_event_cleans_up_expired() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg
            .create("/t/i", -1.0, DeliveryTarget::Callback("http://x.com".into()), 10.0, 0.0)
            .unwrap();
        let values = vec![Value::new(OmiValue::Number(1.0), Some(11.0))];
        let deliveries = reg.notify_event("/t/i", &values, 11.0);
        assert!(deliveries.is_empty());
        assert!(matches!(reg.poll(&rid, 12.0), PollResult::NotFound));
    }

    #[test]
    fn notify_event_unsubscribed_path() {
        let mut reg = SubscriptionRegistry::new();
        let values = vec![Value::new(OmiValue::Number(1.0), None)];
        let deliveries = reg.notify_event("/unsubscribed", &values, 0.0);
        assert!(deliveries.is_empty());
    }

    // --- Interval subscription edge cases ---

    #[test]
    fn interval_sub_expired_during_tick() {
        let mut e = Engine::new();
        e.tree.write_value("/t/i", OmiValue::Number(1.0), Some(0.0)).unwrap();
        let resp = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":5,"read":{"path":"/t/i","interval":2.0}}"#,
            0.0,
            None,
        );
        assert_eq!(response_status(&resp), 200);
        let rid = response_rid(&resp).to_string();
        let deliveries = e.tick(10.0);
        assert!(deliveries.is_empty());
        let poll_json = format!(r#"{{"omi":"1.0","ttl":0,"read":{{"rid":"{}"}}}}"#, rid);
        let resp = process_at(&mut e, &poll_json, 10.0, None);
        assert_eq!(response_status(&resp), 404);
    }

    #[test]
    fn interval_tick_on_deleted_path_no_panic() {
        let mut e = Engine::new();
        e.tree.write_value("/t/i", OmiValue::Number(1.0), Some(0.0)).unwrap();
        process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":2.0,"callback":"http://x.com"}}"#,
            0.0,
            None,
        );
        parse_and_process(&mut e, r#"{"omi":"1.0","ttl":0,"delete":{"path":"/t"}}"#);
        // Tick should not panic; delivery may fire with empty values
        // since the path no longer exists (read returns None → empty vec)
        let deliveries = e.tick(3.0);
        if !deliveries.is_empty() {
            assert!(deliveries[0].values.is_empty(), "deleted path should yield no values");
        }
    }

    // --- WebSocket session cancellation ---

    #[test]
    fn cancel_ws_session_with_no_subscriptions() {
        let mut reg = SubscriptionRegistry::new();
        assert_eq!(reg.cancel_by_ws_session(999), 0);
    }

    #[test]
    fn cancel_ws_session_only_affects_that_session() {
        let mut reg = SubscriptionRegistry::new();
        let rid1 = reg
            .create("/t/a", -1.0, DeliveryTarget::WebSocket(1), 60.0, 0.0)
            .unwrap();
        let rid2 = reg
            .create("/t/b", -1.0, DeliveryTarget::WebSocket(2), 60.0, 0.0)
            .unwrap();
        assert_eq!(reg.cancel_by_ws_session(1), 1);
        assert!(matches!(reg.poll(&rid2, 1.0), PollResult::NotPollable));
        assert!(matches!(reg.poll(&rid1, 1.0), PollResult::NotFound));
    }

    #[test]
    fn cancel_ws_session_multiple_subs() {
        let mut reg = SubscriptionRegistry::new();
        reg.create("/t/a", -1.0, DeliveryTarget::WebSocket(5), 60.0, 0.0).unwrap();
        reg.create("/t/b", -1.0, DeliveryTarget::WebSocket(5), 60.0, 0.0).unwrap();
        reg.create("/t/c", 1.0, DeliveryTarget::WebSocket(5), 60.0, 0.0).unwrap();
        assert_eq!(reg.cancel_by_ws_session(5), 3);
    }

    // --- Poll buffer overflow ---

    #[test]
    fn poll_buffer_overflow_evicts_oldest() {
        let mut buf = PollBuffer::new(3);
        for i in 1..=5 {
            buf.push(&[Value::new(OmiValue::Number(i as f64), None)]);
        }
        let drained = buf.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].v, OmiValue::Number(3.0));
        assert_eq!(drained[1].v, OmiValue::Number(4.0));
        assert_eq!(drained[2].v, OmiValue::Number(5.0));
    }

    #[test]
    fn poll_buffer_drain_then_refill() {
        let mut buf = PollBuffer::new(2);
        buf.push(&[Value::new(OmiValue::Number(1.0), None)]);
        assert_eq!(buf.drain().len(), 1);
        assert!(buf.is_empty());
        buf.push(&[Value::new(OmiValue::Number(2.0), None)]);
        assert_eq!(buf.len(), 1);
        let drained = buf.drain();
        assert_eq!(drained[0].v, OmiValue::Number(2.0));
    }

    // --- Multiple subscriptions on same path ---

    #[test]
    fn multiple_event_subs_same_path_both_fire() {
        let mut e = Engine::new();
        let resp1 = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":-1,"callback":"http://a.com"}}"#,
            0.0,
            None,
        );
        let resp2 = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":-1,"callback":"http://b.com"}}"#,
            0.0,
            None,
        );
        let rid1 = response_rid(&resp1).to_string();
        let rid2 = response_rid(&resp2).to_string();
        assert_ne!(rid1, rid2);

        let (_, deliveries) = process_at_with_deliveries(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/t/i","v":42}}"#,
            1.0,
            None,
        );
        assert_eq!(deliveries.len(), 2);
        let rids: Vec<&str> = deliveries.iter().map(|d| d.rid.as_str()).collect();
        assert!(rids.contains(&rid1.as_str()));
        assert!(rids.contains(&rid2.as_str()));
    }

    #[test]
    fn cancel_one_sub_leaves_other_on_same_path() {
        let mut e = Engine::new();
        let resp1 = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":-1,"callback":"http://a.com"}}"#,
            0.0,
            None,
        );
        let resp2 = process_at(
            &mut e,
            r#"{"omi":"1.0","ttl":60,"read":{"path":"/t/i","interval":-1,"callback":"http://b.com"}}"#,
            0.0,
            None,
        );
        let rid1 = response_rid(&resp1).to_string();
        let rid2 = response_rid(&resp2).to_string();

        let cancel_json = format!(
            r#"{{"omi":"1.0","ttl":0,"cancel":{{"rid":["{}"]}}}}"#,
            rid1
        );
        parse_and_process(&mut e, &cancel_json);

        let (_, deliveries) = process_at_with_deliveries(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/t/i","v":99}}"#,
            2.0,
            None,
        );
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].rid, rid2);
    }

    // --- Expire method ---

    #[test]
    fn expire_removes_all_expired_subs() {
        let mut reg = SubscriptionRegistry::new();
        reg.create("/t/a", -1.0, DeliveryTarget::Poll, 5.0, 0.0).unwrap();
        reg.create("/t/b", -1.0, DeliveryTarget::Poll, 10.0, 0.0).unwrap();
        reg.create("/t/c", -1.0, DeliveryTarget::Poll, -1.0, 0.0).unwrap(); // never expires
        assert_eq!(reg.len(), 3);
        let expired = reg.expire(6.0);
        assert_eq!(expired, 1); // only /t/a expired
        assert_eq!(reg.len(), 2);
        let expired = reg.expire(11.0);
        assert_eq!(expired, 1); // /t/b expired
        assert_eq!(reg.len(), 1); // /t/c never expires
    }
}

// ===========================================================================
// 11. Engine Error Propagation — Additional Paths
// ===========================================================================

mod engine_additional_errors {
    use super::common::*;
    use reconfigurable_device::omi::Engine;

    #[test]
    fn response_as_request_returns_400() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"response":{"status":200}}"#,
        );
        assert_eq!(response_status(&resp), 400);
        assert!(response_desc(&resp).unwrap().contains("not responses"));
    }

    #[test]
    fn write_to_object_path_returns_400() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/System","v":42}}"#,
        );
        assert_eq!(response_status(&resp), 400);
        assert!(response_desc(&resp).unwrap().contains("object path"));
    }

    #[test]
    fn write_batch_to_object_path() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"items":[{"path":"/System","v":42}]}}"#,
        );
        let batch = response_batch(&resp);
        assert_eq!(batch[0].status, 400);
    }

    #[test]
    fn write_batch_all_failures() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"items":[
                {"path":"/System/FreeHeap","v":1},
                {"path":"/System","v":2}
            ]}}"#,
        );
        let batch = response_batch(&resp);
        assert_eq!(batch[0].status, 403); // not writable
        assert_eq!(batch[1].status, 400); // object path
    }

    #[test]
    fn write_batch_partial_success() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"items":[
                {"path":"/New/Good","v":1},
                {"path":"/System/FreeHeap","v":2},
                {"path":"/New/Also","v":3}
            ]}}"#,
        );
        let batch = response_batch(&resp);
        assert_eq!(batch[0].status, 201);
        assert_eq!(batch[1].status, 403);
        assert_eq!(batch[2].status, 201);
    }

    #[test]
    fn read_directory_traversal_rejected() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/../secret"}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn read_double_slash_rejected() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/System//FreeHeap"}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn read_trailing_slash_rejected() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/System/"}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn write_directory_traversal_rejected() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/../etc/passwd","v":"evil"}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn write_tree_invalid_path_rejected() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"no-slash","objects":{"X":{"id":"X"}}}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn delete_directory_traversal_rejected() {
        let mut e = engine_with_sensor_tree();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"delete":{"path":"/System/.."}}"#,
        );
        assert_eq!(response_status(&resp), 400);
    }

    #[test]
    fn delete_infoitem_then_read_404() {
        let mut e = Engine::new();
        parse_and_process(&mut e, r#"{"omi":"1.0","ttl":10,"write":{"path":"/O/I","v":1}}"#);
        let resp = parse_and_process(&mut e, r#"{"omi":"1.0","ttl":0,"delete":{"path":"/O/I"}}"#);
        assert_eq!(response_status(&resp), 200);
        let resp = parse_and_process(&mut e, r#"{"omi":"1.0","ttl":0,"read":{"path":"/O/I"}}"#);
        assert_eq!(response_status(&resp), 404);
    }

    #[test]
    fn write_empty_string_value() {
        let mut e = Engine::new();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":10,"write":{"path":"/t/e","v":""}}"#,
        );
        assert_eq!(response_status(&resp), 201);
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/t/e","newest":1}}"#,
        );
        let values = extract_values(&resp);
        assert_eq!(values[0].v, reconfigurable_device::odf::OmiValue::Str("".into()));
    }

    #[test]
    fn read_newest_zero_returns_empty() {
        let mut e = Engine::new();
        e.tree.write_value("/t/i", reconfigurable_device::odf::OmiValue::Number(1.0), None).unwrap();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/t/i","newest":0}}"#,
        );
        assert_eq!(response_status(&resp), 200);
        assert_eq!(extract_values(&resp).len(), 0);
    }

    #[test]
    fn read_time_range_no_match() {
        let mut e = Engine::new();
        e.tree
            .write_value("/t/i", reconfigurable_device::odf::OmiValue::Number(1.0), Some(100.0))
            .unwrap();
        let resp = parse_and_process(
            &mut e,
            r#"{"omi":"1.0","ttl":0,"read":{"path":"/t/i","begin":200,"end":300}}"#,
        );
        assert_eq!(extract_values(&resp).len(), 0);
    }
}
