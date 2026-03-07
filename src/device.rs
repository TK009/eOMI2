// Device initialization: sensor tree builder and writable-item collector.
//
// Builds the initial O-DF tree representing hardware sensors.
// Also provides helpers for NVS persistence of client-written items.

#[cfg(feature = "std")]
use std::collections::BTreeMap;

#[cfg(feature = "std")]
use crate::odf::{InfoItem, Object, ObjectTree, OmiValue, PathTarget};

/// O-DF path for the free-heap reading.
pub const PATH_FREE_HEAP: &str = "/System/FreeHeap";

/// O-DF path prefix for the discovery subtree.
pub const PATH_DISCOVERY: &str = "/System/discovery";

/// Capacity for sensor InfoItem ring buffers.
const SENSOR_CAPACITY: usize = 20;

/// Build the sensor object tree for internal system metrics.
///
/// Returns a map with a single `System` object containing a read-only
/// `FreeHeap` InfoItem (bytes of free heap memory).  Uses internal
/// counters so no external sensor hardware is required.
#[cfg(feature = "std")]
pub fn build_sensor_tree() -> BTreeMap<String, Object> {
    let mut sys = Object::new("System");
    sys.type_uri = Some("omi:device:system".into());

    let mut heap = InfoItem::new(SENSOR_CAPACITY);
    heap.type_uri = Some("omi:memory:freeheap".into());
    let mut heap_meta = BTreeMap::new();
    heap_meta.insert("unit".into(), OmiValue::Str("B".into()));
    heap.meta = Some(heap_meta);

    sys.add_item("FreeHeap".into(), heap);

    let mut map = BTreeMap::new();
    map.insert("System".into(), sys);
    map
}

/// A single writable item's latest value, for NVS persistence.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedItem {
    pub path: String,
    pub v: OmiValue,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub t: Option<f64>,
}

/// Walk the object tree and collect the newest value from each writable InfoItem.
///
/// Used to persist client-written data to NVS. Sensor-owned (non-writable)
/// items are skipped — they regenerate from code on boot.
#[cfg(feature = "std")]
pub fn collect_writable_items(tree: &ObjectTree) -> Vec<SavedItem> {
    let mut result = Vec::new();
    for (obj_id, obj) in tree.root_objects() {
        collect_from_object(obj, &format!("/{}", obj_id), &mut result);
    }
    result
}

#[cfg(feature = "std")]
fn collect_from_object(obj: &Object, prefix: &str, result: &mut Vec<SavedItem>) {
    if let Some(items) = &obj.items {
        for (name, item) in items {
            if item.is_writable() {
                let path = format!("{}/{}", prefix, name);
                let values = item.query_values(Some(1), None, None, None);
                if let Some(newest) = values.first() {
                    result.push(SavedItem {
                        path,
                        v: newest.v.clone(),
                        t: newest.t,
                    });
                }
            }
        }
    }
    if let Some(children) = &obj.objects {
        for (child_id, child) in children {
            collect_from_object(child, &format!("{}/{}", prefix, child_id), result);
        }
    }
}

/// Update the discovery subtree with the latest mDNS browse results.
///
/// Each peer is written as an InfoItem at `/System/discovery/<hostname>`
/// with value `"<ip>:<port>"` and the given timestamp. Peers not present
/// in the current browse cycle are removed (stale cleanup).
/// Discovery items are sensor-owned (not writable).
///
/// Returns the number of stale peers removed.
#[cfg(feature = "std")]
pub fn update_discovery_tree(
    tree: &mut ObjectTree,
    peers: &[crate::mdns_discovery::Peer],
    now: Option<f64>,
) -> usize {
    // Collect current discovery item names for stale detection
    let current_names: Vec<String> = match tree.resolve(PATH_DISCOVERY) {
        Ok(PathTarget::Object(obj)) => obj
            .items
            .as_ref()
            .map(|items| items.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    // Build set of peer hostnames from this browse cycle
    let live_names: std::collections::HashSet<&str> =
        peers.iter().map(|p| p.hostname.as_str()).collect();

    // Remove stale peers
    let mut removed = 0;
    for name in &current_names {
        if !live_names.contains(name.as_str()) {
            let path = format!("{}/{}", PATH_DISCOVERY, name);
            if tree.delete(&path).is_ok() {
                removed += 1;
            }
        }
    }

    // Write/update current peers
    for peer in peers {
        if peer.hostname.is_empty() {
            continue;
        }
        let path = format!("{}/{}", PATH_DISCOVERY, peer.hostname);
        let value = OmiValue::Str(format!("{}:{}", peer.ip, peer.port));
        let _ = tree.write_value(&path, value, now);
    }

    removed
}

// ---------------------------------------------------------------------------
// NVS serialization helpers (platform-independent, json-gated)
// ---------------------------------------------------------------------------

/// Maximum blob size for NVS storage. Leave headroom below the NVS page
/// size (~4096 bytes) to avoid write failures.
#[cfg(feature = "json")]
pub const MAX_NVS_BLOB: usize = 4000;

/// Errors from serializing items for NVS persistence.
#[cfg(feature = "json")]
#[derive(Debug, PartialEq)]
pub enum NvsSaveError {
    /// Serialized blob exceeds [`MAX_NVS_BLOB`]. Contains the actual size.
    TooLarge(usize),
    SerializeFailed,
}

/// Serialize saved items to JSON bytes, enforcing the NVS blob size limit.
#[cfg(feature = "json")]
pub fn serialize_saved_items(items: &[SavedItem]) -> Result<Vec<u8>, NvsSaveError> {
    let blob = serde_json::to_vec(items).map_err(|_| NvsSaveError::SerializeFailed)?;
    if blob.len() > MAX_NVS_BLOB {
        return Err(NvsSaveError::TooLarge(blob.len()));
    }
    Ok(blob)
}

/// Deserialize saved items from a JSON byte slice.
#[cfg(feature = "json")]
pub fn deserialize_saved_items(data: &[u8]) -> Result<Vec<SavedItem>, String> {
    serde_json::from_slice(data).map_err(|e| e.to_string())
}

#[cfg(test)]
#[cfg(feature = "std")]
mod tests {
    use super::*;
    use crate::odf::PathTarget;

    #[test]
    fn sensor_tree_has_system_object() {
        let tree = build_sensor_tree();
        assert!(tree.contains_key("System"));
        let sys = &tree["System"];
        assert_eq!(sys.id, "System");
    }

    #[test]
    fn sensor_tree_has_free_heap_item() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let heap = sys.get_item("FreeHeap").expect("FreeHeap item missing");
        assert_eq!(heap.type_uri.as_deref(), Some("omi:memory:freeheap"));
        assert_eq!(heap.values.len(), 0);
    }

    #[test]
    fn sensor_items_not_writable() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let heap = sys.get_item("FreeHeap").unwrap();
        assert!(!heap.is_writable());
    }

    #[test]
    fn sensor_items_have_unit_meta() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];

        let heap_meta = sys.get_item("FreeHeap").unwrap().meta.as_ref().unwrap();
        assert_eq!(heap_meta.get("unit"), Some(&OmiValue::Str("B".into())));
    }

    #[test]
    fn collect_returns_empty_for_sensor_only_tree() {
        let mut ot = ObjectTree::new();
        ot.write_tree("/", build_sensor_tree()).unwrap();
        let items = collect_writable_items(&ot);
        assert!(items.is_empty(), "sensor items should not be collected");
    }

    #[test]
    fn collect_finds_engine_written_items() {
        let mut ot = ObjectTree::new();
        ot.write_tree("/", build_sensor_tree()).unwrap();

        // Simulate an engine-created writable item
        ot.write_value("/UserObj/Setting", OmiValue::Number(42.0), Some(1000.0)).unwrap();
        // Mark it writable like the engine would
        if let Ok(crate::odf::PathTargetMut::InfoItem(item)) = ot.resolve_mut("/UserObj/Setting") {
            let meta = item.meta.get_or_insert_with(BTreeMap::new);
            meta.insert("writable".into(), OmiValue::Bool(true));
        }

        let items = collect_writable_items(&ot);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "/UserObj/Setting");
        assert_eq!(items[0].v, OmiValue::Number(42.0));
        assert_eq!(items[0].t, Some(1000.0));
    }

    #[test]
    fn collect_skips_non_writable_user_items() {
        let mut ot = ObjectTree::new();
        // Create an item without marking writable
        ot.write_value("/Obj/Item", OmiValue::Str("hello".into()), None).unwrap();
        let items = collect_writable_items(&ot);
        assert!(items.is_empty());
    }

    #[test]
    fn collect_handles_nested_writable_items() {
        let mut ot = ObjectTree::new();
        ot.write_value("/A/B/C/D", OmiValue::Number(1.0), Some(500.0)).unwrap();
        if let Ok(crate::odf::PathTargetMut::InfoItem(item)) = ot.resolve_mut("/A/B/C/D") {
            let meta = item.meta.get_or_insert_with(BTreeMap::new);
            meta.insert("writable".into(), OmiValue::Bool(true));
        }

        let items = collect_writable_items(&ot);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "/A/B/C/D");
    }

    #[test]
    fn path_constants_match_tree() {
        let tree = build_sensor_tree();
        let mut ot = ObjectTree::new();
        ot.write_tree("/", tree).unwrap();

        assert!(matches!(ot.resolve(PATH_FREE_HEAP), Ok(PathTarget::InfoItem(_))));
    }

    // --- serialize_saved_items ---

    #[test]
    fn serialize_empty() {
        let blob = serialize_saved_items(&[]).unwrap();
        assert_eq!(blob, b"[]");
    }

    #[test]
    fn serialize_single_item() {
        let items = vec![SavedItem {
            path: "/A/B".into(),
            v: OmiValue::Number(42.0),
            t: Some(1000.0),
        }];
        let blob = serialize_saved_items(&items).unwrap();
        let text = std::str::from_utf8(&blob).unwrap();
        assert!(text.contains("\"path\":\"/A/B\""));
        assert!(text.contains("42"));
    }

    #[test]
    fn serialize_too_large() {
        // Create items whose serialized form exceeds MAX_NVS_BLOB
        let items: Vec<SavedItem> = (0..500)
            .map(|i| SavedItem {
                path: format!("/Object{}/LongItemName{}", i, i),
                v: OmiValue::Str("x".repeat(20)),
                t: Some(i as f64),
            })
            .collect();
        let err = serialize_saved_items(&items).unwrap_err();
        match err {
            NvsSaveError::TooLarge(size) => assert!(size > MAX_NVS_BLOB),
            other => panic!("expected TooLarge, got {:?}", other),
        }
    }

    #[test]
    fn serialize_under_limit() {
        let items = vec![
            SavedItem { path: "/A/X".into(), v: OmiValue::Number(1.0), t: None },
            SavedItem { path: "/A/Y".into(), v: OmiValue::Bool(true), t: Some(99.0) },
        ];
        let blob = serialize_saved_items(&items).unwrap();
        assert!(blob.len() <= MAX_NVS_BLOB);
    }

    // --- deserialize_saved_items ---

    #[test]
    fn deserialize_empty_array() {
        let items = deserialize_saved_items(b"[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn deserialize_single_item() {
        // OmiValue is serde(untagged), so strings serialize as bare JSON strings
        let json = r#"[{"path":"/A/B","v":"hello","t":1000.0}]"#;
        let items = deserialize_saved_items(json.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "/A/B");
        assert_eq!(items[0].v, OmiValue::Str("hello".into()));
        assert_eq!(items[0].t, Some(1000.0));
    }

    #[test]
    fn deserialize_no_timestamp() {
        let json = r#"[{"path":"/X","v":3.14}]"#;
        let items = deserialize_saved_items(json.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].t, None);
    }

    #[test]
    fn deserialize_invalid_json() {
        assert!(deserialize_saved_items(b"not json").is_err());
    }

    #[test]
    fn deserialize_wrong_structure() {
        // Valid JSON but not an array of SavedItem
        assert!(deserialize_saved_items(b"{}").is_err());
    }

    // --- roundtrip ---

    #[test]
    fn serialize_deserialize_roundtrip() {
        let items = vec![
            SavedItem { path: "/A/B".into(), v: OmiValue::Number(42.0), t: Some(1000.0) },
            SavedItem { path: "/C/D".into(), v: OmiValue::Str("hello".into()), t: None },
            SavedItem { path: "/E/F".into(), v: OmiValue::Bool(true), t: Some(2000.0) },
        ];
        let blob = serialize_saved_items(&items).unwrap();
        let restored = deserialize_saved_items(&blob).unwrap();
        assert_eq!(items, restored);
    }

    // --- update_discovery_tree ---

    use crate::mdns_discovery::Peer;

    fn make_peer(hostname: &str, ip: &str, port: u16) -> Peer {
        Peer { hostname: hostname.into(), ip: ip.into(), port }
    }

    #[test]
    fn discovery_writes_peers_to_tree() {
        let mut tree = ObjectTree::new();
        let peers = vec![
            make_peer("kitchen", "192.168.1.10", 80),
            make_peer("garage", "192.168.1.11", 8080),
        ];
        update_discovery_tree(&mut tree, &peers, Some(1000.0));

        match tree.resolve("/System/discovery/kitchen") {
            Ok(PathTarget::InfoItem(item)) => {
                let vals = item.query_values(Some(1), None, None, None);
                assert_eq!(vals[0].v, OmiValue::Str("192.168.1.10:80".into()));
                assert_eq!(vals[0].t, Some(1000.0));
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }

        match tree.resolve("/System/discovery/garage") {
            Ok(PathTarget::InfoItem(item)) => {
                let vals = item.query_values(Some(1), None, None, None);
                assert_eq!(vals[0].v, OmiValue::Str("192.168.1.11:8080".into()));
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }

    #[test]
    fn discovery_removes_stale_peers() {
        let mut tree = ObjectTree::new();

        // First cycle: two peers
        let peers1 = vec![
            make_peer("kitchen", "192.168.1.10", 80),
            make_peer("garage", "192.168.1.11", 80),
        ];
        update_discovery_tree(&mut tree, &peers1, Some(1000.0));

        // Second cycle: only kitchen remains
        let peers2 = vec![make_peer("kitchen", "192.168.1.10", 80)];
        let removed = update_discovery_tree(&mut tree, &peers2, Some(2000.0));

        assert_eq!(removed, 1);
        assert!(tree.resolve("/System/discovery/kitchen").is_ok());
        assert!(tree.resolve("/System/discovery/garage").is_err());
    }

    #[test]
    fn discovery_empty_peers_clears_all() {
        let mut tree = ObjectTree::new();
        let peers = vec![make_peer("kitchen", "192.168.1.10", 80)];
        update_discovery_tree(&mut tree, &peers, Some(1000.0));

        let removed = update_discovery_tree(&mut tree, &[], Some(2000.0));
        assert_eq!(removed, 1);
        // Discovery object may still exist but has no items
    }

    #[test]
    fn discovery_updates_existing_peer_value() {
        let mut tree = ObjectTree::new();

        // First cycle
        let peers1 = vec![make_peer("kitchen", "192.168.1.10", 80)];
        update_discovery_tree(&mut tree, &peers1, Some(1000.0));

        // Second cycle: same peer, different IP
        let peers2 = vec![make_peer("kitchen", "192.168.1.99", 80)];
        update_discovery_tree(&mut tree, &peers2, Some(2000.0));

        match tree.resolve("/System/discovery/kitchen") {
            Ok(PathTarget::InfoItem(item)) => {
                let vals = item.query_values(Some(1), None, None, None);
                assert_eq!(vals[0].v, OmiValue::Str("192.168.1.99:80".into()));
                assert_eq!(vals[0].t, Some(2000.0));
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }

    #[test]
    fn discovery_skips_empty_hostname() {
        let mut tree = ObjectTree::new();
        let peers = vec![make_peer("", "192.168.1.10", 80)];
        update_discovery_tree(&mut tree, &peers, Some(1000.0));

        // No items should be created for empty hostname
        assert!(tree.resolve("/System/discovery").is_err());
    }

    #[test]
    fn discovery_items_not_writable() {
        let mut tree = ObjectTree::new();
        let peers = vec![make_peer("kitchen", "192.168.1.10", 80)];
        update_discovery_tree(&mut tree, &peers, Some(1000.0));

        match tree.resolve("/System/discovery/kitchen") {
            Ok(PathTarget::InfoItem(item)) => {
                assert!(!item.is_writable());
            }
            other => panic!("expected InfoItem, got {:?}", other),
        }
    }

    #[test]
    fn discovery_on_empty_tree_no_panic() {
        let mut tree = ObjectTree::new();
        let removed = update_discovery_tree(&mut tree, &[], None);
        assert_eq!(removed, 0);
    }
}
