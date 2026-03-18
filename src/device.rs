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

/// O-DF path for free flash memory reading.
#[cfg(feature = "mem-stats")]
pub const PATH_FREE_FLASH: &str = "/System/FreeFlash";

/// O-DF path for free ODF storage reading.
#[cfg(feature = "mem-stats")]
pub const PATH_FREE_ODF_STORAGE: &str = "/System/FreeOdfStorage";

/// O-DF path for free PSRAM reading.
#[cfg(all(feature = "mem-stats", feature = "psram"))]
pub const PATH_FREE_PSRAM: &str = "/System/FreePsram";

/// O-DF path for the temperature sensor reading.
pub const PATH_TEMPERATURE: &str = "/System/Temperature";

/// O-DF path for the firmware version InfoItem (FR-023).
pub const PATH_FIRMWARE_VERSION: &str = "/System/FirmwareVersion";

/// O-DF path prefix for the discovery subtree.
pub const PATH_DISCOVERY: &str = "/System/discovery";

/// Capacity for sensor InfoItem ring buffers.
const SENSOR_CAPACITY: usize = 20;

/// Compile-time firmware version string (FR-024).
const FIRMWARE_VERSION: &str = env!("FIRMWARE_VERSION");

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
    heap_meta.insert("total".into(), OmiValue::Number(0.0));
    heap.meta = Some(heap_meta);
    sys.add_item("FreeHeap".into(), heap);

    #[cfg(feature = "mem-stats")]
    {
        let mut flash = InfoItem::new(SENSOR_CAPACITY);
        flash.type_uri = Some("omi:memory:freeflash".into());
        let mut flash_meta = BTreeMap::new();
        flash_meta.insert("unit".into(), OmiValue::Str("B".into()));
        flash_meta.insert("total".into(), OmiValue::Number(0.0));
        flash.meta = Some(flash_meta);
        sys.add_item("FreeFlash".into(), flash);

        let mut odf = InfoItem::new(SENSOR_CAPACITY);
        odf.type_uri = Some("omi:memory:freeodf".into());
        let mut odf_meta = BTreeMap::new();
        odf_meta.insert("unit".into(), OmiValue::Str("B".into()));
        odf_meta.insert("total".into(), OmiValue::Number(0.0));
        odf.meta = Some(odf_meta);
        sys.add_item("FreeOdfStorage".into(), odf);
    }

    #[cfg(all(feature = "mem-stats", feature = "psram"))]
    {
        let mut psram = InfoItem::new(SENSOR_CAPACITY);
        psram.type_uri = Some("omi:memory:freepsram".into());
        let mut psram_meta = BTreeMap::new();
        psram_meta.insert("unit".into(), OmiValue::Str("B".into()));
        psram_meta.insert("total".into(), OmiValue::Number(0.0));
        psram.meta = Some(psram_meta);
        sys.add_item("FreePsram".into(), psram);
    }

    // FR-023: read-only firmware version InfoItem.
    let mut fw = InfoItem::new(1);
    fw.type_uri = Some("omi:device:firmwareversion".into());
    fw.add_value(OmiValue::Str(FIRMWARE_VERSION.into()), None);
    sys.add_item("FirmwareVersion".into(), fw);

    if crate::board::has_temp_sensor() {
        let mut temp = InfoItem::new(SENSOR_CAPACITY);
        temp.type_uri = Some("omi:sensor:temperature".into());
        let mut temp_meta = BTreeMap::new();
        temp_meta.insert("unit".into(), OmiValue::Str("°C".into()));
        temp.meta = Some(temp_meta);
        sys.add_item("Temperature".into(), temp);
    }

    let mut map = BTreeMap::new();
    map.insert("System".into(), sys);
    map
}

/// A single writable item's latest value, for NVS persistence.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedItem {
    pub path: String,
    pub v: OmiValue,
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
// NVS serialization helpers (platform-independent, compact binary format)
// ---------------------------------------------------------------------------

/// Maximum blob size for NVS storage. Leave headroom below the NVS page
/// size (~4096 bytes) to avoid write failures.
pub const MAX_NVS_BLOB: usize = 4000;

/// Errors from serializing items for NVS persistence.
#[derive(Debug, PartialEq)]
pub enum NvsSaveError {
    /// Serialized blob exceeds [`MAX_NVS_BLOB`]. Contains the actual size.
    TooLarge(usize),
    SerializeFailed,
}

/// Binary format version tag.
const SAVED_ITEMS_VERSION: u8 = 0x01;

/// Serialize saved items to compact binary, enforcing the NVS blob size limit.
///
/// Wire format:
/// ```text
/// [version: u8 = 0x01] [item_count: u16-LE]
/// per item:
///   [path_len: u16-LE] [path: utf8]
///   [type_tag: u8]  -- 0=null, 1=bool(0), 2=bool(1), 3=f64, 4=str
///   [value: variable] -- f64: 8 bytes LE; str: u16-LE len + bytes
///   [has_t: u8] [t: f64-LE if has_t=1]
/// ```
pub fn serialize_saved_items(items: &[SavedItem]) -> Result<Vec<u8>, NvsSaveError> {
    let mut buf = Vec::with_capacity(128);
    buf.push(SAVED_ITEMS_VERSION);
    let count: u16 = items.len().try_into().map_err(|_| NvsSaveError::SerializeFailed)?;
    buf.extend_from_slice(&count.to_le_bytes());

    for item in items {
        let path_bytes = item.path.as_bytes();
        let path_len: u16 = path_bytes.len().try_into().map_err(|_| NvsSaveError::SerializeFailed)?;
        buf.extend_from_slice(&path_len.to_le_bytes());
        buf.extend_from_slice(path_bytes);

        match &item.v {
            OmiValue::Null => buf.push(0),
            OmiValue::Bool(false) => buf.push(1),
            OmiValue::Bool(true) => buf.push(2),
            OmiValue::Number(n) => {
                buf.push(3);
                buf.extend_from_slice(&n.to_le_bytes());
            }
            OmiValue::Str(s) => {
                buf.push(4);
                let s_bytes = s.as_bytes();
                let s_len: u16 = s_bytes.len().try_into().map_err(|_| NvsSaveError::SerializeFailed)?;
                buf.extend_from_slice(&s_len.to_le_bytes());
                buf.extend_from_slice(s_bytes);
            }
        }

        match item.t {
            Some(t) => {
                buf.push(1);
                buf.extend_from_slice(&t.to_le_bytes());
            }
            None => buf.push(0),
        }
    }

    if buf.len() > MAX_NVS_BLOB {
        return Err(NvsSaveError::TooLarge(buf.len()));
    }
    Ok(buf)
}

/// Deserialize saved items from a compact binary byte slice.
pub fn deserialize_saved_items(data: &[u8]) -> Result<Vec<SavedItem>, String> {
    let mut pos = 0;

    let read_u8 = |pos: &mut usize| -> Result<u8, String> {
        if *pos >= data.len() {
            return Err("unexpected end of data".into());
        }
        let v = data[*pos];
        *pos += 1;
        Ok(v)
    };

    let read_u16 = |pos: &mut usize| -> Result<u16, String> {
        if *pos + 2 > data.len() {
            return Err("unexpected end of data".into());
        }
        let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
        *pos += 2;
        Ok(v)
    };

    let read_f64 = |pos: &mut usize| -> Result<f64, String> {
        if *pos + 8 > data.len() {
            return Err("unexpected end of data".into());
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&data[*pos..*pos + 8]);
        *pos += 8;
        Ok(f64::from_le_bytes(bytes))
    };

    let version = read_u8(&mut pos)?;
    if version != SAVED_ITEMS_VERSION {
        return Err(format!("unsupported version: {}", version));
    }

    let count = read_u16(&mut pos)? as usize;
    let mut items = Vec::with_capacity(count);

    for _ in 0..count {
        let path_len = read_u16(&mut pos)? as usize;
        if pos + path_len > data.len() {
            return Err("unexpected end of data".into());
        }
        let path = core::str::from_utf8(&data[pos..pos + path_len])
            .map_err(|e| e.to_string())?
            .to_string();
        pos += path_len;

        let tag = read_u8(&mut pos)?;
        let v = match tag {
            0 => OmiValue::Null,
            1 => OmiValue::Bool(false),
            2 => OmiValue::Bool(true),
            3 => OmiValue::Number(read_f64(&mut pos)?),
            4 => {
                let s_len = read_u16(&mut pos)? as usize;
                if pos + s_len > data.len() {
                    return Err("unexpected end of data".into());
                }
                let s = core::str::from_utf8(&data[pos..pos + s_len])
                    .map_err(|e| e.to_string())?
                    .to_string();
                pos += s_len;
                OmiValue::Str(s)
            }
            _ => return Err(format!("unknown type tag: {}", tag)),
        };

        let has_t = read_u8(&mut pos)?;
        let t = if has_t == 1 {
            Some(read_f64(&mut pos)?)
        } else {
            None
        };

        items.push(SavedItem { path, v, t });
    }

    Ok(items)
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
        assert_eq!(heap_meta.get("total"), Some(&OmiValue::Number(0.0)));
    }

    #[cfg(feature = "mem-stats")]
    #[test]
    fn sensor_tree_has_free_flash() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let item = sys.get_item("FreeFlash").expect("FreeFlash missing");
        assert_eq!(item.type_uri.as_deref(), Some("omi:memory:freeflash"));
        let meta = item.meta.as_ref().unwrap();
        assert_eq!(meta.get("unit"), Some(&OmiValue::Str("B".into())));
        assert_eq!(meta.get("total"), Some(&OmiValue::Number(0.0)));
    }

    #[cfg(feature = "mem-stats")]
    #[test]
    fn sensor_tree_has_free_odf_storage() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let item = sys.get_item("FreeOdfStorage").expect("FreeOdfStorage missing");
        assert_eq!(item.type_uri.as_deref(), Some("omi:memory:freeodf"));
        let meta = item.meta.as_ref().unwrap();
        assert_eq!(meta.get("unit"), Some(&OmiValue::Str("B".into())));
        assert_eq!(meta.get("total"), Some(&OmiValue::Number(0.0)));
    }

    #[cfg(all(feature = "mem-stats", feature = "psram"))]
    #[test]
    fn sensor_tree_has_free_psram() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let item = sys.get_item("FreePsram").expect("FreePsram missing");
        assert_eq!(item.type_uri.as_deref(), Some("omi:memory:freepsram"));
        let meta = item.meta.as_ref().unwrap();
        assert_eq!(meta.get("unit"), Some(&OmiValue::Str("B".into())));
        assert_eq!(meta.get("total"), Some(&OmiValue::Number(0.0)));
    }

    #[cfg(feature = "mem-stats")]
    #[test]
    fn mem_stats_path_constants_match_tree() {
        let tree = build_sensor_tree();
        let mut ot = ObjectTree::new();
        ot.write_tree("/", tree).unwrap();

        assert!(matches!(ot.resolve(PATH_FREE_FLASH), Ok(PathTarget::InfoItem(_))));
        assert!(matches!(ot.resolve(PATH_FREE_ODF_STORAGE), Ok(PathTarget::InfoItem(_))));
    }

    #[cfg(all(feature = "mem-stats", feature = "psram"))]
    #[test]
    fn psram_path_constant_matches_tree() {
        let tree = build_sensor_tree();
        let mut ot = ObjectTree::new();
        ot.write_tree("/", tree).unwrap();

        assert!(matches!(ot.resolve(PATH_FREE_PSRAM), Ok(PathTarget::InfoItem(_))));
    }

    #[test]
    fn system_object_has_type_uri() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        assert_eq!(sys.type_uri.as_deref(), Some("omi:device:system"));
    }

    #[test]
    fn heap_item_has_total_meta() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let meta = sys.get_item("FreeHeap").unwrap().meta.as_ref().unwrap();
        assert!(meta.contains_key("total"), "FreeHeap must have meta.total");
    }

    #[cfg(feature = "mem-stats")]
    #[test]
    fn mem_stats_items_not_writable() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let flash = sys.get_item("FreeFlash").unwrap();
        assert!(!flash.is_writable(), "FreeFlash should not be writable");
        let odf = sys.get_item("FreeOdfStorage").unwrap();
        assert!(!odf.is_writable(), "FreeOdfStorage should not be writable");
    }

    #[cfg(feature = "mem-stats")]
    #[test]
    fn mem_stats_items_all_have_total_meta() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        for name in &["FreeHeap", "FreeFlash", "FreeOdfStorage"] {
            let item = sys.get_item(name).unwrap_or_else(|| panic!("{} missing", name));
            let meta = item.meta.as_ref().unwrap_or_else(|| panic!("{} has no meta", name));
            assert!(meta.contains_key("total"), "{} missing meta.total", name);
        }
    }

    #[cfg(not(feature = "mem-stats"))]
    #[test]
    fn without_mem_stats_no_flash_or_odf_items() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        assert!(sys.get_item("FreeFlash").is_none(), "FreeFlash should not exist without mem-stats");
        assert!(sys.get_item("FreeOdfStorage").is_none(), "FreeOdfStorage should not exist without mem-stats");
    }

    #[test]
    fn temperature_gated_by_board_config() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        if crate::board::has_temp_sensor() {
            let temp = sys.get_item("Temperature").expect("Temperature missing with temp sensor");
            assert_eq!(temp.type_uri.as_deref(), Some("omi:sensor:temperature"));
            assert!(!temp.is_writable(), "Temperature should not be writable");
            let meta = temp.meta.as_ref().expect("Temperature has no meta");
            assert_eq!(meta.get("unit"), Some(&OmiValue::Str("°C".into())));

            // PATH_TEMPERATURE should resolve
            let mut ot = ObjectTree::new();
            ot.write_tree("/", build_sensor_tree()).unwrap();
            assert!(matches!(ot.resolve(PATH_TEMPERATURE), Ok(PathTarget::InfoItem(_))));
        } else {
            assert!(sys.get_item("Temperature").is_none(),
                "Temperature should not exist when board has no temp sensor");
        }
    }

    #[cfg(all(feature = "mem-stats", feature = "psram"))]
    #[test]
    fn psram_item_not_writable() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let psram = sys.get_item("FreePsram").unwrap();
        assert!(!psram.is_writable(), "FreePsram should not be writable");
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

    #[test]
    fn sensor_tree_has_firmware_version() {
        let tree = build_sensor_tree();
        let sys = &tree["System"];
        let fw = sys.get_item("FirmwareVersion").expect("FirmwareVersion missing");
        assert_eq!(fw.type_uri.as_deref(), Some("omi:device:firmwareversion"));
        assert!(!fw.is_writable(), "FirmwareVersion should not be writable");
        // Should have exactly one value pre-populated
        let vals = fw.query_values(Some(1), None, None, None);
        assert_eq!(vals.len(), 1);
        assert!(matches!(&vals[0].v, OmiValue::Str(_)));
    }

    #[test]
    fn firmware_version_path_resolves() {
        let tree = build_sensor_tree();
        let mut ot = ObjectTree::new();
        ot.write_tree("/", tree).unwrap();
        assert!(matches!(ot.resolve(PATH_FIRMWARE_VERSION), Ok(PathTarget::InfoItem(_))));
    }

    // --- serialize/deserialize_saved_items (compact binary format) ---

    mod binary_persistence {
        use super::*;

        #[test]
        fn serialize_empty() {
            let blob = serialize_saved_items(&[]).unwrap();
            // version(1) + count(2) = 3 bytes
            assert_eq!(blob.len(), 3);
            assert_eq!(blob[0], 0x01); // version
            assert_eq!(blob[1..3], [0, 0]); // count = 0
        }

        #[test]
        fn serialize_single_item() {
            let items = vec![SavedItem {
                path: "/A/B".into(),
                v: OmiValue::Number(42.0),
                t: Some(1000.0),
            }];
            let blob = serialize_saved_items(&items).unwrap();
            assert_eq!(blob[0], 0x01); // version
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(restored, items);
        }

        #[test]
        fn serialize_too_large() {
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

        #[test]
        fn deserialize_empty_items() {
            let blob = serialize_saved_items(&[]).unwrap();
            let items = deserialize_saved_items(&blob).unwrap();
            assert!(items.is_empty());
        }

        #[test]
        fn deserialize_invalid_data() {
            assert!(deserialize_saved_items(b"not binary").is_err());
        }

        #[test]
        fn deserialize_wrong_version() {
            assert!(deserialize_saved_items(&[0xFF, 0, 0]).is_err());
        }

        #[test]
        fn deserialize_truncated() {
            // Valid version but truncated count
            assert!(deserialize_saved_items(&[0x01]).is_err());
        }

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

        #[test]
        fn roundtrip_null_value() {
            let items = vec![SavedItem {
                path: "/X/Y".into(),
                v: OmiValue::Null,
                t: None,
            }];
            let blob = serialize_saved_items(&items).unwrap();
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(items, restored);
        }

        #[test]
        fn roundtrip_all_value_types() {
            let items = vec![
                SavedItem { path: "/null".into(), v: OmiValue::Null, t: None },
                SavedItem { path: "/false".into(), v: OmiValue::Bool(false), t: Some(1.0) },
                SavedItem { path: "/true".into(), v: OmiValue::Bool(true), t: None },
                SavedItem { path: "/num".into(), v: OmiValue::Number(3.14), t: Some(2.0) },
                SavedItem { path: "/str".into(), v: OmiValue::Str("test".into()), t: None },
            ];
            let blob = serialize_saved_items(&items).unwrap();
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(items, restored);
        }

        #[test]
        fn roundtrip_unicode_strings() {
            let items = vec![
                SavedItem { path: "/日本語/パス".into(), v: OmiValue::Str("こんにちは".into()), t: None },
                SavedItem { path: "/emoji".into(), v: OmiValue::Str("🌡️".into()), t: Some(1.0) },
            ];
            let blob = serialize_saved_items(&items).unwrap();
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(items, restored);
        }

        #[test]
        fn roundtrip_empty_path_and_string() {
            let items = vec![
                SavedItem { path: "".into(), v: OmiValue::Str(String::new()), t: None },
            ];
            let blob = serialize_saved_items(&items).unwrap();
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(items, restored);
        }

        #[test]
        fn roundtrip_max_length_path() {
            let long_path = "/".to_string() + &"a".repeat(1000);
            let items = vec![SavedItem {
                path: long_path.clone(),
                v: OmiValue::Number(0.0),
                t: None,
            }];
            let blob = serialize_saved_items(&items).unwrap();
            let restored = deserialize_saved_items(&blob).unwrap();
            assert_eq!(restored[0].path, long_path);
        }

        // --- Adversarial / error-path tests ---

        #[test]
        fn deserialize_future_version_tag() {
            // Version 0x02 — a hypothetical future version should be rejected gracefully
            assert!(deserialize_saved_items(&[0x02, 0, 0]).unwrap_err().contains("unsupported version"));
            // Version 0x00 — below current
            assert!(deserialize_saved_items(&[0x00, 0, 0]).unwrap_err().contains("unsupported version"));
            // High version byte
            assert!(deserialize_saved_items(&[0x7F, 0, 0]).unwrap_err().contains("unsupported version"));
        }

        #[test]
        fn deserialize_truncated_mid_path_length() {
            // Valid header (version + count=1) but path_len is truncated (only 1 byte of u16)
            let data = [0x01, 1, 0, 0x05]; // version=1, count=1, path_len starts but only 1 byte
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_mid_path_data() {
            // version=1, count=1, path_len=10, but only 3 bytes of path follow
            let mut data = vec![0x01, 1, 0]; // version + count=1
            data.extend_from_slice(&10u16.to_le_bytes()); // path_len=10
            data.extend_from_slice(b"abc"); // only 3 bytes of path
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_before_type_tag() {
            // version=1, count=1, valid path, then EOF before type tag
            let mut data = vec![0x01, 1, 0]; // version + count=1
            data.extend_from_slice(&2u16.to_le_bytes()); // path_len=2
            data.extend_from_slice(b"/A"); // path
            // Missing type tag
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_mid_f64_value() {
            // version=1, count=1, path="/A", type=Number(3), then only 4 of 8 f64 bytes
            let mut data = vec![0x01, 1, 0]; // version + count=1
            data.extend_from_slice(&2u16.to_le_bytes()); // path_len=2
            data.extend_from_slice(b"/A"); // path
            data.push(3); // type_tag = Number
            data.extend_from_slice(&[0u8; 4]); // only 4 bytes of f64 (need 8)
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_mid_string_length() {
            // type=Str(4), then only 1 byte of string length u16
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(4); // type_tag = Str
            data.push(0x05); // only 1 byte of string length
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_mid_string_data() {
            // type=Str, string_len=10, but only 3 bytes of string data
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(4); // type_tag = Str
            data.extend_from_slice(&10u16.to_le_bytes()); // string_len=10
            data.extend_from_slice(b"abc"); // only 3 bytes
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_before_has_t() {
            // Complete value but EOF before has_t byte
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(0); // type_tag = Null
            // Missing has_t byte
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_truncated_mid_timestamp() {
            // has_t=1 but only 4 of 8 timestamp bytes
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(0); // Null
            data.push(1); // has_t = true
            data.extend_from_slice(&[0u8; 4]); // only 4 bytes of f64
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_unknown_type_tag() {
            // type_tag = 5 (unknown — only 0-4 are valid)
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(5); // unknown type tag
            let err = deserialize_saved_items(&data).unwrap_err();
            assert!(err.contains("unknown type tag"), "got: {}", err);
        }

        #[test]
        fn deserialize_unknown_type_tag_high() {
            // type_tag = 0xFF
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(0xFF);
            assert!(deserialize_saved_items(&data).unwrap_err().contains("unknown type tag"));
        }

        #[test]
        fn deserialize_count_exceeds_data() {
            // Claims 1000 items but has no item data
            let mut data = vec![0x01];
            data.extend_from_slice(&1000u16.to_le_bytes());
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_invalid_utf8_path() {
            // Path contains invalid UTF-8
            let mut data = vec![0x01, 1, 0]; // version + count=1
            data.extend_from_slice(&3u16.to_le_bytes()); // path_len=3
            data.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // invalid UTF-8
            data.push(0); // type_tag = Null
            data.push(0); // has_t = false
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_invalid_utf8_string_value() {
            // String value contains invalid UTF-8
            let mut data = vec![0x01, 1, 0];
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A"); // valid path
            data.push(4); // type_tag = Str
            data.extend_from_slice(&3u16.to_le_bytes()); // string_len=3
            data.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // invalid UTF-8
            data.push(0); // has_t = false
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn deserialize_corrupt_first_item_valid_second() {
            // Two items claimed, but first has corrupt type tag — should fail on first
            let mut data = vec![0x01, 2, 0]; // version + count=2
            data.extend_from_slice(&2u16.to_le_bytes());
            data.extend_from_slice(b"/A");
            data.push(99); // bad type tag on first item
            // Second item would be valid but we never reach it
            assert!(deserialize_saved_items(&data).is_err());
        }

        #[test]
        fn binary_is_compact() {
            // Verify binary is more compact than equivalent JSON would be
            let items = vec![
                SavedItem { path: "/A/B".into(), v: OmiValue::Number(42.0), t: Some(1000.0) },
                SavedItem { path: "/C/D".into(), v: OmiValue::Str("hello".into()), t: None },
            ];
            let blob = serialize_saved_items(&items).unwrap();
            // Binary should be well under 100 bytes for this
            assert!(blob.len() < 100, "binary blob unexpectedly large: {} bytes", blob.len());
        }
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
