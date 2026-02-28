use std::collections::BTreeMap;

#[cfg(feature = "json")]
use serde::{Deserialize, Serialize};

use super::OmiValue;
use super::value::{RingBuffer, Value};

/// An InfoItem in the O-DF object tree.
///
/// Holds a named measurement or control point with a history of timestamped values.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct InfoItem {
    #[cfg_attr(feature = "json", serde(rename = "type", skip_serializing_if = "Option::is_none"))]
    pub type_uri: Option<String>,

    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub desc: Option<String>,

    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub meta: Option<BTreeMap<String, OmiValue>>,

    #[cfg_attr(feature = "json", serde(skip_serializing_if = "RingBuffer::is_empty"))]
    pub values: RingBuffer,
}

impl InfoItem {
    pub fn new(capacity: usize) -> Self {
        Self {
            type_uri: None,
            desc: None,
            meta: None,
            values: RingBuffer::new(capacity),
        }
    }

    /// Check if this InfoItem is writable. Defaults to false.
    pub fn is_writable(&self) -> bool {
        self.meta
            .as_ref()
            .and_then(|m| m.get("writable"))
            .map_or(false, |v| matches!(v, OmiValue::Bool(true)))
    }

    /// Check if this InfoItem is readable. Defaults to true.
    pub fn is_readable(&self) -> bool {
        self.meta
            .as_ref()
            .and_then(|m| m.get("readable"))
            .map_or(true, |v| !matches!(v, OmiValue::Bool(false)))
    }

    /// Add a new value to the ring buffer.
    pub fn add_value(&mut self, v: OmiValue, t: Option<f64>) {
        self.values.push(Value::new(v, t));
    }

    /// Query values with optional count/time filters.
    pub fn query_values(
        &self,
        newest: Option<usize>,
        oldest: Option<usize>,
        begin: Option<f64>,
        end: Option<f64>,
    ) -> Vec<Value> {
        self.values.query(newest, oldest, begin, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_add_values() {
        let mut item = InfoItem::new(10);
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        item.add_value(OmiValue::Number(23.0), Some(1001.0));
        assert_eq!(item.values.len(), 2);

        let result = item.query_values(Some(1), None, None, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].v, OmiValue::Number(23.0));
    }

    #[test]
    fn writable_default_false() {
        let item = InfoItem::new(10);
        assert!(!item.is_writable());
    }

    #[test]
    fn writable_with_meta() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("writable".into(), OmiValue::Bool(true));
        item.meta = Some(meta);
        assert!(item.is_writable());
    }

    #[test]
    fn writable_false_explicit() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("writable".into(), OmiValue::Bool(false));
        item.meta = Some(meta);
        assert!(!item.is_writable());
    }

    #[test]
    fn readable_default_true() {
        let item = InfoItem::new(10);
        assert!(item.is_readable());
    }

    #[test]
    fn readable_false() {
        let mut item = InfoItem::new(10);
        let mut meta = BTreeMap::new();
        meta.insert("readable".into(), OmiValue::Bool(false));
        item.meta = Some(meta);
        assert!(!item.is_readable());
    }

    #[test]
    fn query_with_time_range() {
        let mut item = InfoItem::new(100);
        for i in 0..10 {
            item.add_value(OmiValue::Number(i as f64), Some(i as f64 * 100.0));
        }
        let result = item.query_values(None, None, Some(300.0), Some(600.0));
        assert_eq!(result.len(), 4); // timestamps 300, 400, 500, 600
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn serialize_empty_item() {
            let item = InfoItem::new(10);
            let json = serde_json::to_value(&item).unwrap();
            // Empty item should have no values key (skip_serializing_if)
            assert!(json.get("values").is_none());
            assert!(json.get("type").is_none());
            assert!(json.get("desc").is_none());
            assert!(json.get("meta").is_none());
        }

        #[test]
        fn serialize_with_values() {
            let mut item = InfoItem::new(10);
            item.type_uri = Some("omi:temperature".into());
            item.desc = Some("Room temperature".into());
            item.add_value(OmiValue::Number(22.5), Some(1000.0));

            let json = serde_json::to_value(&item).unwrap();
            assert_eq!(json["type"], "omi:temperature");
            assert_eq!(json["desc"], "Room temperature");
            let values = json["values"].as_array().unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0]["v"], 22.5);
            assert_eq!(values[0]["t"], 1000.0);
        }

        #[test]
        fn deserialize_item() {
            let json = r#"{
                "type": "omi:temperature",
                "desc": "Sensor reading",
                "values": [
                    {"v": 23.0, "t": 1001.0},
                    {"v": 22.5, "t": 1000.0}
                ]
            }"#;
            let item: InfoItem = serde_json::from_str(json).unwrap();
            assert_eq!(item.type_uri.as_deref(), Some("omi:temperature"));
            assert_eq!(item.desc.as_deref(), Some("Sensor reading"));
            assert_eq!(item.values.len(), 2);
        }
    }
}
