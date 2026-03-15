use std::collections::BTreeMap;

use crate::odf::{OmiValue, Object};

/// Maximum nesting depth for Object trees in write operations.
/// Limits recursive serde deserialization to stay within the HTTP thread stack.
/// NOTE: If you change this value, also update `HTTP_THREAD_STACK` in `server.rs`.
pub(crate) const MAX_OBJECT_DEPTH: usize = 8;

/// Count the maximum Object nesting depth in a parsed objects map, iteratively.
///
/// Returns 0 if the map is empty.
pub(crate) fn parsed_object_tree_depth(objects: &BTreeMap<String, Object>) -> usize {
    let mut max_depth: usize = 0;
    let mut stack: Vec<(&Object, usize)> = Vec::new();
    for obj in objects.values() {
        stack.push((obj, 1));
    }
    while let Some((obj, depth)) = stack.pop() {
        if depth > max_depth {
            max_depth = depth;
        }
        if let Some(children) = &obj.objects {
            for child in children.values() {
                stack.push((child, depth + 1));
            }
        }
    }
    max_depth
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteOp {
    Single {
        path: String,
        v: OmiValue,
        t: Option<f64>,
    },
    Batch {
        items: Vec<WriteItem>,
    },
    Tree {
        path: String,
        objects: BTreeMap<String, Object>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteItem {
    pub path: String,
    pub v: OmiValue,
    pub t: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    #[test]
    fn write_item_fields() {
        let item = WriteItem {
            path: "/A/B".into(),
            v: OmiValue::Number(42.0),
            t: Some(1000.0),
        };
        assert_eq!(item.path, "/A/B");
        assert_eq!(item.v, OmiValue::Number(42.0));
        assert_eq!(item.t, Some(1000.0));
    }

}
