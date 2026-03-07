use std::collections::BTreeMap;
use std::fmt;

use super::item::InfoItem;
use super::object::Object;
use super::OmiValue;

#[derive(Debug, PartialEq)]
pub enum TreeError {
    NotFound(String),
    Forbidden(String),
    InvalidPath(String),
    #[cfg(feature = "json")]
    SerializationError(String),
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TreeError::NotFound(msg) => write!(f, "Not found: {}", msg),
            TreeError::Forbidden(msg) => write!(f, "Forbidden: {}", msg),
            TreeError::InvalidPath(msg) => write!(f, "Invalid path: {}", msg),
            #[cfg(feature = "json")]
            TreeError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for TreeError {}

/// Immutable reference to a resolved path target.
#[derive(Debug)]
pub enum PathTarget<'a> {
    Root(&'a BTreeMap<String, Object>),
    Object(&'a Object),
    InfoItem(&'a InfoItem),
}

/// Mutable reference to a resolved path target.
pub enum PathTargetMut<'a> {
    Root(&'a mut BTreeMap<String, Object>),
    Object(&'a mut Object),
    InfoItem(&'a mut InfoItem),
}

/// Parse a path string into segments. Validates the path.
///
/// - The path must start with `/`.
/// - `".."` segments are rejected.
/// - Empty segments (from double slashes or trailing slashes) are rejected.
///   Note: trailing slashes like `"/DeviceA/"` produce an empty last segment
///   and will return `InvalidPath`. Callers should strip trailing slashes
///   before calling this function if they want to tolerate them.
fn parse_path(path: &str) -> Result<Vec<&str>, TreeError> {
    if !path.starts_with('/') {
        return Err(TreeError::InvalidPath(
            "Path must start with '/'".into(),
        ));
    }

    if path == "/" {
        return Ok(vec![]);
    }

    let segments: Vec<&str> = path[1..]
        .split('/')
        .map(|s| s.trim())
        .collect();

    for seg in &segments {
        if seg.is_empty() {
            return Err(TreeError::InvalidPath(
                "Empty segment (double slash or trailing slash)".into(),
            ));
        }
        if *seg == ".." {
            return Err(TreeError::InvalidPath(
                "'..' segments not allowed".into(),
            ));
        }
    }

    Ok(segments)
}

/// The root object tree. Entry point for path-based operations.
pub struct ObjectTree {
    objects: BTreeMap<String, Object>,
    /// Default ring buffer capacity for auto-created InfoItems.
    default_capacity: usize,
}

/// Default ring buffer capacity for auto-created InfoItems.
const DEFAULT_ITEM_CAPACITY: usize = 100;

impl ObjectTree {
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            default_capacity: DEFAULT_ITEM_CAPACITY,
        }
    }

    pub fn with_capacity(default_capacity: usize) -> Self {
        Self {
            objects: BTreeMap::new(),
            default_capacity,
        }
    }

    /// Iterate over root-level objects.
    pub fn root_objects(&self) -> impl Iterator<Item = (&str, &Object)> {
        self.objects.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Insert an object at root level, keyed by its `id`.
    pub fn insert_root(&mut self, obj: Object) -> Option<Object> {
        self.objects.insert(obj.id.clone(), obj)
    }

    /// True if the tree has no root-level objects.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// True if a root-level object with the given id exists.
    pub fn root_contains(&self, id: &str) -> bool {
        self.objects.contains_key(id)
    }

    /// Collect all (path, onread_script) pairs from the tree.
    ///
    /// Used by the scripting engine to pre-compile onread functions before
    /// execution, avoiding re-entrant mJS compilation.
    #[cfg(feature = "scripting")]
    pub fn collect_onread_scripts(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for (_, obj) in &self.objects {
            Self::collect_onread_from_object(obj, &format!("/{}", obj.id), &mut result);
        }
        result
    }

    #[cfg(feature = "scripting")]
    fn collect_onread_from_object(obj: &Object, prefix: &str, out: &mut Vec<(String, String)>) {
        if let Some(items) = &obj.items {
            for (name, item) in items {
                if let Some(script) = item.get_onread_script() {
                    out.push((format!("{}/{}", prefix, name), script.to_string()));
                }
            }
        }
        if let Some(children) = &obj.objects {
            for (id, child) in children {
                Self::collect_onread_from_object(child, &format!("{}/{}", prefix, id), out);
            }
        }
    }

    /// Resolve an immutable reference from a path.
    pub fn resolve(&self, path: &str) -> Result<PathTarget<'_>, TreeError> {
        let segments = parse_path(path)?;

        if segments.is_empty() {
            return Ok(PathTarget::Root(&self.objects));
        }

        let first = segments[0];
        let obj = self.objects.get(first).ok_or_else(|| {
            TreeError::NotFound(format!("Object '{}' not found", first))
        })?;

        if segments.len() == 1 {
            return Ok(PathTarget::Object(obj));
        }

        Self::resolve_from_object(obj, &segments[1..])
    }

    /// Walk from an object through remaining path segments.
    ///
    /// When resolving the **last** segment, InfoItems take precedence over
    /// child Objects if both share the same name. This matches the common
    /// case where leaf paths target sensor values rather than sub-objects.
    fn resolve_from_object<'a>(
        obj: &'a Object,
        segments: &[&str],
    ) -> Result<PathTarget<'a>, TreeError> {
        if segments.is_empty() {
            return Ok(PathTarget::Object(obj));
        }

        let name = segments[0];

        // Last segment: InfoItem takes precedence over child Object
        if segments.len() == 1 {
            if let Some(item) = obj.get_item(name) {
                return Ok(PathTarget::InfoItem(item));
            }
            if let Some(child) = obj.get_child(name) {
                return Ok(PathTarget::Object(child));
            }
            return Err(TreeError::NotFound(format!(
                "'{}' not found in object '{}'",
                name, obj.id
            )));
        }

        // Not the last segment: must be a child Object
        let child = obj.get_child(name).ok_or_else(|| {
            TreeError::NotFound(format!("Object '{}' not found in '{}'", name, obj.id))
        })?;

        Self::resolve_from_object(child, &segments[1..])
    }

    /// Resolve a mutable reference from a path.
    pub fn resolve_mut(&mut self, path: &str) -> Result<PathTargetMut<'_>, TreeError> {
        let segments = parse_path(path)?;

        if segments.is_empty() {
            return Ok(PathTargetMut::Root(&mut self.objects));
        }

        let first = segments[0].to_string();
        let obj = self.objects.get_mut(&first).ok_or_else(|| {
            TreeError::NotFound(format!("Object '{}' not found", first))
        })?;

        if segments.len() == 1 {
            return Ok(PathTargetMut::Object(obj));
        }

        Self::resolve_from_object_mut(obj, &segments[1..])
    }

    /// Mutable version of [`resolve_from_object`]. Same precedence rules apply:
    /// InfoItems take precedence over child Objects for the last segment.
    fn resolve_from_object_mut<'a>(
        obj: &'a mut Object,
        segments: &[&str],
    ) -> Result<PathTargetMut<'a>, TreeError> {
        if segments.is_empty() {
            return Ok(PathTargetMut::Object(obj));
        }

        let name = segments[0];
        let obj_id = obj.id.clone();

        // Last segment: InfoItem takes precedence over child Object
        if segments.len() == 1 {
            // Check existence first to avoid borrow issues
            let has_item = obj.get_item(name).is_some();
            let has_child = obj.get_child(name).is_some();

            if has_item {
                return Ok(PathTargetMut::InfoItem(
                    obj.get_item_mut(name).unwrap(),
                ));
            }
            if has_child {
                return Ok(PathTargetMut::Object(
                    obj.get_child_mut(name).unwrap(),
                ));
            }
            return Err(TreeError::NotFound(format!(
                "'{}' not found in object '{}'",
                name, obj_id
            )));
        }

        // Not the last segment: must be a child Object
        let child = obj.get_child_mut(name).ok_or_else(|| {
            TreeError::NotFound(format!("Object '{}' not found in '{}'", name, obj_id))
        })?;

        Self::resolve_from_object_mut(child, &segments[1..])
    }

    /// Write a single value to a path. Auto-creates objects/items as needed.
    /// Returns true if the path was newly created (201), false if it existed (200).
    ///
    /// Auto-created InfoItems use `self.default_capacity` as the ring buffer size.
    pub fn write_value(
        &mut self,
        path: &str,
        v: OmiValue,
        t: Option<f64>,
    ) -> Result<bool, TreeError> {
        let segments = parse_path(path)?;

        if segments.is_empty() {
            return Err(TreeError::InvalidPath(
                "Cannot write a value to root".into(),
            ));
        }

        if segments.len() < 2 {
            return Err(TreeError::InvalidPath(
                "Value path must have at least an object and an item (e.g. /Obj/Item)".into(),
            ));
        }

        // Walk/create objects for all segments except the last (which is the InfoItem)
        let item_name = segments.last().unwrap().to_string();
        let obj_segments = &segments[..segments.len() - 1];

        let mut created = false;

        // Ensure the first object exists
        let first = obj_segments[0].to_string();
        if !self.objects.contains_key(&first) {
            self.objects.insert(first.clone(), Object::new(&first));
            created = true;
        }

        // Walk into nested objects, creating as needed
        let mut current = self.objects.get_mut(&first).unwrap();
        for &seg in &obj_segments[1..] {
            let has_child = current.get_child(seg).is_some();
            if !has_child {
                current.add_child(Object::new(seg));
                created = true;
            }
            current = current.get_child_mut(seg).unwrap();
        }

        // Now current is the parent object. Add or get the InfoItem.
        let has_item = current.get_item(&item_name).is_some();
        if !has_item {
            current.add_item(item_name.clone(), InfoItem::new(self.default_capacity));
            created = true;
        }

        let item = current.get_item_mut(&item_name).unwrap();
        item.add_value(v, t);

        Ok(created)
    }

    /// Merge an object tree at the given path.
    ///
    /// Each object is inserted using its `obj.id` field as the key, ensuring
    /// consistency between map keys and object identities.
    pub fn write_tree(
        &mut self,
        path: &str,
        objects: BTreeMap<String, Object>,
    ) -> Result<(), TreeError> {
        let segments = parse_path(path)?;

        if segments.is_empty() {
            // Merge at root
            for (_, obj) in objects {
                let id = obj.id.clone();
                self.objects.insert(id, obj);
            }
            return Ok(());
        }

        // Walk to the target object, creating as needed
        let first = segments[0].to_string();
        if !self.objects.contains_key(&first) {
            self.objects.insert(first.clone(), Object::new(&first));
        }

        let mut current = self.objects.get_mut(&first).unwrap();
        for &seg in &segments[1..] {
            let has_child = current.get_child(seg).is_some();
            if !has_child {
                current.add_child(Object::new(seg));
            }
            current = current.get_child_mut(seg).unwrap();
        }

        // Merge children into the target object
        for (_, obj) in objects {
            current.add_child(obj);
        }

        Ok(())
    }

    /// Delete an object or InfoItem at the given path.
    pub fn delete(&mut self, path: &str) -> Result<(), TreeError> {
        let segments = parse_path(path)?;

        if segments.is_empty() {
            return Err(TreeError::Forbidden("Cannot delete root".into()));
        }

        if segments.len() == 1 {
            let name = segments[0];
            if self.objects.remove(name).is_some() {
                return Ok(());
            }
            return Err(TreeError::NotFound(format!(
                "'{}' not found at root",
                name
            )));
        }

        // Walk to the parent object
        let target_name = segments.last().unwrap().to_string();
        let parent_segments = &segments[..segments.len() - 1];

        let first = parent_segments[0].to_string();
        let parent_obj = self.objects.get_mut(&first).ok_or_else(|| {
            TreeError::NotFound(format!("Object '{}' not found", first))
        })?;

        let parent = if parent_segments.len() == 1 {
            parent_obj
        } else {
            Self::walk_to_object_mut(parent_obj, &parent_segments[1..])?
        };

        // Try removing as InfoItem first, then as child Object
        if parent.remove_item(&target_name).is_some() {
            return Ok(());
        }
        if parent.remove_child(&target_name).is_some() {
            return Ok(());
        }

        Err(TreeError::NotFound(format!(
            "'{}' not found in '{}'",
            target_name, parent.id
        )))
    }

    fn walk_to_object_mut<'a>(
        obj: &'a mut Object,
        segments: &[&str],
    ) -> Result<&'a mut Object, TreeError> {
        let mut current = obj;
        for &seg in segments {
            let id = current.id.clone();
            current = current.get_child_mut(seg).ok_or_else(|| {
                TreeError::NotFound(format!("Object '{}' not found in '{}'", seg, id))
            })?;
        }
        Ok(current)
    }

    /// Read a subtree as JSON with an optional depth limit.
    #[cfg(feature = "json")]
    pub fn read(
        &self,
        path: &str,
        depth: Option<usize>,
    ) -> Result<serde_json::Value, TreeError> {
        let target = self.resolve(path)?;

        match target {
            PathTarget::Root(objects) => {
                let mut map = serde_json::Map::new();
                for (id, obj) in objects {
                    let val = match depth {
                        Some(d) => obj.serialize_with_depth(d)
                            .map_err(|e| TreeError::SerializationError(e.to_string()))?,
                        None => serde_json::to_value(obj)
                            .map_err(|e| TreeError::SerializationError(e.to_string()))?,
                    };
                    map.insert(id.clone(), val);
                }
                Ok(serde_json::Value::Object(map))
            }
            PathTarget::Object(obj) => {
                let val = match depth {
                    Some(d) => obj.serialize_with_depth(d)
                        .map_err(|e| TreeError::SerializationError(e.to_string()))?,
                    None => serde_json::to_value(obj)
                        .map_err(|e| TreeError::SerializationError(e.to_string()))?,
                };
                Ok(val)
            }
            PathTarget::InfoItem(item) => {
                serde_json::to_value(item)
                    .map_err(|e| TreeError::SerializationError(e.to_string()))
            }
        }
    }
}

impl Default for ObjectTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    // --- Path parsing tests ---

    #[test]
    fn parse_root() {
        let segs = parse_path("/").unwrap();
        assert!(segs.is_empty());
    }

    #[test]
    fn parse_single_segment() {
        let segs = parse_path("/A").unwrap();
        assert_eq!(segs, vec!["A"]);
    }

    #[test]
    fn parse_multi_segment() {
        let segs = parse_path("/A/B/C").unwrap();
        assert_eq!(segs, vec!["A", "B", "C"]);
    }

    #[test]
    fn parse_rejects_no_leading_slash() {
        assert!(parse_path("A/B").is_err());
    }

    #[test]
    fn parse_rejects_double_slash() {
        assert!(parse_path("/A//B").is_err());
    }

    #[test]
    fn parse_rejects_dotdot() {
        assert!(parse_path("/A/../B").is_err());
    }

    #[test]
    fn parse_rejects_trailing_slash() {
        assert!(parse_path("/A/B/").is_err());
    }

    // --- Resolve tests ---

    fn sample_tree() -> ObjectTree {
        let mut tree = ObjectTree::new();
        let mut device = Object::new("DeviceA");
        let mut sub = Object::new("SubDevice");

        let mut temp = InfoItem::new(10);
        temp.add_value(OmiValue::Number(22.5), Some(1000.0));
        device.add_item("Temperature".into(), temp);

        sub.add_item("Voltage".into(), InfoItem::new(10));
        device.add_child(sub);
        tree.insert_root(device);
        tree
    }

    #[test]
    fn resolve_root() {
        let tree = sample_tree();
        match tree.resolve("/").unwrap() {
            PathTarget::Root(objs) => assert!(objs.contains_key("DeviceA")),
            _ => panic!("expected Root"),
        }
    }

    #[test]
    fn resolve_object() {
        let tree = sample_tree();
        match tree.resolve("/DeviceA").unwrap() {
            PathTarget::Object(obj) => assert_eq!(obj.id, "DeviceA"),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn resolve_info_item() {
        let tree = sample_tree();
        match tree.resolve("/DeviceA/Temperature").unwrap() {
            PathTarget::InfoItem(item) => assert_eq!(item.values.len(), 1),
            _ => panic!("expected InfoItem"),
        }
    }

    #[test]
    fn resolve_nested_object() {
        let tree = sample_tree();
        match tree.resolve("/DeviceA/SubDevice").unwrap() {
            PathTarget::Object(obj) => assert_eq!(obj.id, "SubDevice"),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn resolve_nested_item() {
        let tree = sample_tree();
        match tree.resolve("/DeviceA/SubDevice/Voltage").unwrap() {
            PathTarget::InfoItem(_) => {}
            _ => panic!("expected InfoItem"),
        }
    }

    #[test]
    fn resolve_not_found() {
        let tree = sample_tree();
        assert_eq!(
            tree.resolve("/Missing").unwrap_err(),
            TreeError::NotFound("Object 'Missing' not found".into())
        );
    }

    // --- Write value tests ---

    #[test]
    fn write_value_to_new_path() {
        let mut tree = ObjectTree::new();
        let created = tree
            .write_value("/DeviceA/Temperature", OmiValue::Number(22.5), Some(1000.0))
            .unwrap();
        assert!(created);

        match tree.resolve("/DeviceA/Temperature").unwrap() {
            PathTarget::InfoItem(item) => {
                assert_eq!(item.values.len(), 1);
            }
            _ => panic!("expected InfoItem"),
        }
    }

    #[test]
    fn write_value_to_existing_path() {
        let mut tree = ObjectTree::new();
        tree.write_value("/DeviceA/Temperature", OmiValue::Number(22.5), Some(1000.0))
            .unwrap();
        let created = tree
            .write_value("/DeviceA/Temperature", OmiValue::Number(23.0), Some(1001.0))
            .unwrap();
        assert!(!created);

        match tree.resolve("/DeviceA/Temperature").unwrap() {
            PathTarget::InfoItem(item) => {
                assert_eq!(item.values.len(), 2);
            }
            _ => panic!("expected InfoItem"),
        }
    }

    #[test]
    fn write_value_deep_path() {
        let mut tree = ObjectTree::new();
        tree.write_value(
            "/Building/Floor1/Room101/Temperature",
            OmiValue::Number(21.0),
            None,
        )
        .unwrap();

        match tree.resolve("/Building/Floor1/Room101/Temperature").unwrap() {
            PathTarget::InfoItem(item) => assert_eq!(item.values.len(), 1),
            _ => panic!("expected InfoItem"),
        }
    }

    #[test]
    fn write_value_to_root_rejected() {
        let mut tree = ObjectTree::new();
        assert_eq!(
            tree.write_value("/", OmiValue::Null, None).unwrap_err(),
            TreeError::InvalidPath("Cannot write a value to root".into())
        );
    }

    #[test]
    fn write_value_uses_custom_capacity() {
        let mut tree = ObjectTree::with_capacity(5);
        // Write 10 values — only last 5 should survive
        for i in 0..10 {
            tree.write_value("/D/S", OmiValue::Number(i as f64), Some(i as f64))
                .unwrap();
        }
        match tree.resolve("/D/S").unwrap() {
            PathTarget::InfoItem(item) => {
                assert_eq!(item.values.len(), 5);
                let newest = item.query_values(Some(1), None, None, None);
                assert_eq!(newest[0].v, OmiValue::Number(9.0));
            }
            _ => panic!("expected InfoItem"),
        }
    }

    // --- Write tree tests ---

    #[test]
    fn write_tree_at_root() {
        let mut tree = ObjectTree::new();
        let mut objects = BTreeMap::new();
        let mut dev = Object::new("DeviceB");
        dev.add_item("Humidity".into(), InfoItem::new(10));
        objects.insert("DeviceB".into(), dev);

        tree.write_tree("/", objects).unwrap();
        assert!(tree.root_contains("DeviceB"));
    }

    #[test]
    fn write_tree_at_path() {
        let mut tree = ObjectTree::new();
        tree.insert_root(Object::new("Root"));

        let mut objects = BTreeMap::new();
        objects.insert("Child".into(), Object::new("Child"));
        tree.write_tree("/Root", objects).unwrap();

        match tree.resolve("/Root/Child").unwrap() {
            PathTarget::Object(obj) => assert_eq!(obj.id, "Child"),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn write_tree_uses_obj_id_as_key() {
        let mut tree = ObjectTree::new();
        let mut objects = BTreeMap::new();
        // Deliberately use a different map key than obj.id
        let obj = Object::new("RealId");
        objects.insert("wrong_key".into(), obj);

        tree.write_tree("/", objects).unwrap();
        // Should be stored under obj.id, not the map key
        assert!(tree.root_contains("RealId"));
        assert!(!tree.root_contains("wrong_key"));
    }

    // --- Delete tests ---

    #[test]
    fn delete_root_rejected() {
        let mut tree = ObjectTree::new();
        assert_eq!(
            tree.delete("/").unwrap_err(),
            TreeError::Forbidden("Cannot delete root".into())
        );
    }

    #[test]
    fn delete_top_level_object() {
        let mut tree = sample_tree();
        tree.delete("/DeviceA").unwrap();
        assert!(tree.is_empty());
    }

    #[test]
    fn delete_info_item() {
        let mut tree = sample_tree();
        tree.delete("/DeviceA/Temperature").unwrap();
        match tree.resolve("/DeviceA").unwrap() {
            PathTarget::Object(obj) => assert!(obj.get_item("Temperature").is_none()),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn delete_nested_object() {
        let mut tree = sample_tree();
        tree.delete("/DeviceA/SubDevice").unwrap();
        match tree.resolve("/DeviceA").unwrap() {
            PathTarget::Object(obj) => assert!(obj.get_child("SubDevice").is_none()),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn delete_not_found() {
        let mut tree = sample_tree();
        assert_eq!(
            tree.delete("/DeviceA/Missing").unwrap_err(),
            TreeError::NotFound("'Missing' not found in 'DeviceA'".into())
        );
    }

    // --- Read tests ---

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn read_root() {
            let tree = sample_tree();
            let val = tree.read("/", None).unwrap();
            assert!(val["DeviceA"].is_object());
        }

        #[test]
        fn read_object() {
            let tree = sample_tree();
            let val = tree.read("/DeviceA", None).unwrap();
            assert_eq!(val["id"], "DeviceA");
        }

        #[test]
        fn read_info_item() {
            let tree = sample_tree();
            let val = tree.read("/DeviceA/Temperature", None).unwrap();
            let values = val["values"].as_array().unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0]["v"], 22.5);
        }

        #[test]
        fn read_with_depth_limit() {
            let tree = sample_tree();
            let val = tree.read("/DeviceA", Some(0)).unwrap();
            assert_eq!(val["id"], "DeviceA");
            assert!(val.get("items").is_none());
            assert!(val.get("objects").is_none());
        }

        #[test]
        fn full_scenario_read() {
            let mut tree = ObjectTree::new();
            tree.write_value("/Sensor1/Temperature", OmiValue::Number(20.0), Some(100.0))
                .unwrap();
            tree.write_value("/Sensor1/Humidity", OmiValue::Number(45.0), Some(100.0))
                .unwrap();

            let obj_json = tree.read("/Sensor1", None).unwrap();
            assert_eq!(obj_json["id"], "Sensor1");
            assert!(obj_json["items"]["Temperature"].is_object());
            assert!(obj_json["items"]["Humidity"].is_object());
        }
    }

    // --- Full scenario test ---

    #[test]
    fn full_scenario() {
        let mut tree = ObjectTree::new();

        // Create tree via writes
        tree.write_value("/Sensor1/Temperature", OmiValue::Number(20.0), Some(100.0))
            .unwrap();
        tree.write_value("/Sensor1/Temperature", OmiValue::Number(21.0), Some(200.0))
            .unwrap();
        tree.write_value("/Sensor1/Temperature", OmiValue::Number(22.0), Some(300.0))
            .unwrap();
        tree.write_value("/Sensor1/Humidity", OmiValue::Number(45.0), Some(100.0))
            .unwrap();

        // Verify tree structure via resolve
        match tree.resolve("/Sensor1").unwrap() {
            PathTarget::Object(obj) => {
                assert!(obj.get_item("Temperature").is_some());
                assert!(obj.get_item("Humidity").is_some());
            }
            _ => panic!("expected Object"),
        }

        // Query values
        match tree.resolve("/Sensor1/Temperature").unwrap() {
            PathTarget::InfoItem(item) => {
                let vals = item.query_values(Some(2), None, None, None);
                assert_eq!(vals.len(), 2);
                assert_eq!(vals[0].v, OmiValue::Number(22.0));
                assert_eq!(vals[1].v, OmiValue::Number(21.0));
            }
            _ => panic!("expected InfoItem"),
        }

        // Delete an item
        tree.delete("/Sensor1/Humidity").unwrap();
        assert_eq!(
            tree.resolve("/Sensor1/Humidity").unwrap_err(),
            TreeError::NotFound("'Humidity' not found in object 'Sensor1'".into())
        );

        // Temperature still there
        assert!(matches!(
            tree.resolve("/Sensor1/Temperature"),
            Ok(PathTarget::InfoItem(_))
        ));
    }

    #[test]
    fn ring_buffer_overflow_preserves_newest() {
        let mut tree = ObjectTree::new();

        // Write 150 values to an item (default capacity 100)
        for i in 0..150 {
            tree.write_value(
                "/Device/Sensor",
                OmiValue::Number(i as f64),
                Some(i as f64),
            )
            .unwrap();
        }

        match tree.resolve("/Device/Sensor").unwrap() {
            PathTarget::InfoItem(item) => {
                assert_eq!(item.values.len(), 100);
                let newest = item.query_values(Some(1), None, None, None);
                assert_eq!(newest[0].v, OmiValue::Number(149.0));
                let oldest = item.query_values(None, Some(1), None, None);
                assert_eq!(oldest[0].v, OmiValue::Number(50.0));
            }
            _ => panic!("expected InfoItem"),
        }
    }
}
