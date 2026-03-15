use std::collections::BTreeMap;

use super::item::InfoItem;

/// An Object node in the O-DF hierarchy.
///
/// Contains child Objects and InfoItems. Mirrors the OMI-Lite JSON structure.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub id: String,

    pub type_uri: Option<String>,

    pub desc: Option<String>,

    pub items: Option<BTreeMap<String, InfoItem>>,

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

}
