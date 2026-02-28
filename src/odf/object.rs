use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::item::InfoItem;

/// An Object node in the O-DF hierarchy.
///
/// Contains child Objects and InfoItems. Mirrors the OMI-Lite JSON structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    pub id: String,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_uri: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<BTreeMap<String, InfoItem>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub objects: Option<BTreeMap<String, Object>>,
}

impl Object {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            type_uri: None,
            desc: None,
            items: None,
            objects: None,
        }
    }

    pub fn get_item(&self, name: &str) -> Option<&InfoItem> {
        self.items.as_ref()?.get(name)
    }

    pub fn get_item_mut(&mut self, name: &str) -> Option<&mut InfoItem> {
        self.items.as_mut()?.get_mut(name)
    }

    pub fn get_child(&self, id: &str) -> Option<&Object> {
        self.objects.as_ref()?.get(id)
    }

    pub fn get_child_mut(&mut self, id: &str) -> Option<&mut Object> {
        self.objects.as_mut()?.get_mut(id)
    }

    pub fn add_item(&mut self, name: String, item: InfoItem) {
        self.items.get_or_insert_with(BTreeMap::new).insert(name, item);
    }

    pub fn add_child(&mut self, obj: Object) {
        let id = obj.id.clone();
        self.objects.get_or_insert_with(BTreeMap::new).insert(id, obj);
    }

    pub fn remove_item(&mut self, name: &str) -> Option<InfoItem> {
        let items = self.items.as_mut()?;
        let removed = items.remove(name);
        if items.is_empty() {
            self.items = None;
        }
        removed
    }

    pub fn remove_child(&mut self, id: &str) -> Option<Object> {
        let objects = self.objects.as_mut()?;
        let removed = objects.remove(id);
        if objects.is_empty() {
            self.objects = None;
        }
        removed
    }

    /// Serialize this object with a depth limit.
    ///
    /// Depth 0 = only id/type/desc, no items or child objects.
    /// Depth 1 = include direct items and child object shells (depth 0).
    /// etc.
    pub fn serialize_with_depth(&self, depth: usize) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("id".into(), serde_json::Value::String(self.id.clone()));

        if let Some(ref t) = self.type_uri {
            map.insert("type".into(), serde_json::Value::String(t.clone()));
        }
        if let Some(ref d) = self.desc {
            map.insert("desc".into(), serde_json::Value::String(d.clone()));
        }

        if depth > 0 {
            if let Some(ref items) = self.items {
                let items_val = serde_json::to_value(items).unwrap_or_default();
                if !items.is_empty() {
                    map.insert("items".into(), items_val);
                }
            }
            if let Some(ref objects) = self.objects {
                let mut objs_map = serde_json::Map::new();
                for (k, obj) in objects {
                    objs_map.insert(k.clone(), obj.serialize_with_depth(depth - 1));
                }
                if !objs_map.is_empty() {
                    map.insert("objects".into(), serde_json::Value::Object(objs_map));
                }
            }
        }

        serde_json::Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    fn make_temp_item() -> InfoItem {
        let mut item = InfoItem::new(10);
        item.type_uri = Some("omi:temperature".into());
        item.add_value(OmiValue::Number(22.5), Some(1000.0));
        item
    }

    #[test]
    fn create_empty_object() {
        let obj = Object::new("DeviceA");
        assert_eq!(obj.id, "DeviceA");
        assert!(obj.items.is_none());
        assert!(obj.objects.is_none());
    }

    #[test]
    fn add_and_get_item() {
        let mut obj = Object::new("DeviceA");
        obj.add_item("Temperature".into(), make_temp_item());

        let item = obj.get_item("Temperature").unwrap();
        assert_eq!(item.type_uri.as_deref(), Some("omi:temperature"));
        assert!(obj.get_item("Nonexistent").is_none());
    }

    #[test]
    fn add_and_get_child() {
        let mut parent = Object::new("Root");
        let child = Object::new("Child1");
        parent.add_child(child);

        assert!(parent.get_child("Child1").is_some());
        assert!(parent.get_child("Missing").is_none());
    }

    #[test]
    fn remove_item() {
        let mut obj = Object::new("DeviceA");
        obj.add_item("Temp".into(), make_temp_item());
        let removed = obj.remove_item("Temp");
        assert!(removed.is_some());
        assert!(obj.items.is_none()); // cleaned up empty map
        assert!(obj.remove_item("Temp").is_none());
    }

    #[test]
    fn remove_child() {
        let mut obj = Object::new("Root");
        obj.add_child(Object::new("A"));
        let removed = obj.remove_child("A");
        assert!(removed.is_some());
        assert!(obj.objects.is_none());
        assert!(obj.remove_child("A").is_none());
    }

    #[test]
    fn nested_objects() {
        let mut root = Object::new("Root");
        let mut sub = Object::new("Sub");
        sub.add_item("Voltage".into(), InfoItem::new(10));
        root.add_child(sub);

        let s = root.get_child("Sub").unwrap();
        assert!(s.get_item("Voltage").is_some());
    }

    #[test]
    fn get_item_mut() {
        let mut obj = Object::new("D");
        obj.add_item("Temp".into(), InfoItem::new(10));
        let item = obj.get_item_mut("Temp").unwrap();
        item.add_value(OmiValue::Number(99.0), None);
        assert_eq!(obj.get_item("Temp").unwrap().values.len(), 1);
    }

    #[test]
    fn serialize_basic() {
        let mut obj = Object::new("DeviceA");
        obj.type_uri = Some("omi:device".into());
        obj.add_item("Temperature".into(), make_temp_item());

        let json = serde_json::to_value(&obj).unwrap();
        assert_eq!(json["id"], "DeviceA");
        assert_eq!(json["type"], "omi:device");
        assert!(json["items"]["Temperature"].is_object());
    }

    #[test]
    fn depth_limited_serialization() {
        let mut root = Object::new("Root");
        let mut child = Object::new("Child");
        child.add_item("Temp".into(), make_temp_item());
        root.add_child(child);

        // Depth 0: no items or objects
        let d0 = root.serialize_with_depth(0);
        assert_eq!(d0["id"], "Root");
        assert!(d0.get("items").is_none());
        assert!(d0.get("objects").is_none());

        // Depth 1: includes child shells
        let d1 = root.serialize_with_depth(1);
        assert!(d1["objects"]["Child"].is_object());
        // Child at depth 0 has no items
        assert!(d1["objects"]["Child"].get("items").is_none());

        // Depth 2: full tree
        let d2 = root.serialize_with_depth(2);
        assert!(d2["objects"]["Child"]["items"]["Temp"].is_object());
    }

    #[test]
    fn deserialize_object() {
        let json = r#"{
            "id": "DeviceA",
            "type": "omi:device",
            "items": {
                "Temperature": {
                    "type": "omi:temperature",
                    "values": [{"v": 22.5, "t": 1000.0}]
                }
            },
            "objects": {
                "SubDevice": {
                    "id": "SubDevice"
                }
            }
        }"#;
        let obj: Object = serde_json::from_str(json).unwrap();
        assert_eq!(obj.id, "DeviceA");
        assert!(obj.get_item("Temperature").is_some());
        assert!(obj.get_child("SubDevice").is_some());
    }
}
