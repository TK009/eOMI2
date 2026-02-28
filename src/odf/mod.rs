pub mod value;
pub mod item;
pub mod object;
pub mod tree;

#[cfg(feature = "json")]
use serde::{Deserialize, Serialize};

/// OMI-Lite value type. Covers the common JSON primitives.
/// The spec allows "any JSON type" for `v`, but in practice
/// IoT values are numbers, strings, or booleans.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "json", serde(untagged))]
pub enum OmiValue {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
}

// Re-export key types
pub use value::{Value, RingBuffer};
pub use item::InfoItem;
pub use object::Object;
pub use tree::{ObjectTree, PathTarget, PathTargetMut, TreeError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omi_value_equality() {
        assert_eq!(OmiValue::Number(1.0), OmiValue::Number(1.0));
        assert_ne!(OmiValue::Number(1.0), OmiValue::Number(2.0));
        assert_ne!(OmiValue::Bool(true), OmiValue::Str("true".into()));
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn omi_value_serialize() {
            assert_eq!(serde_json::to_string(&OmiValue::Null).unwrap(), "null");
            assert_eq!(serde_json::to_string(&OmiValue::Bool(true)).unwrap(), "true");
            assert_eq!(serde_json::to_string(&OmiValue::Number(42.5)).unwrap(), "42.5");
            assert_eq!(serde_json::to_string(&OmiValue::Str("hello".into())).unwrap(), "\"hello\"");
        }

        #[test]
        fn omi_value_deserialize() {
            assert_eq!(serde_json::from_str::<OmiValue>("null").unwrap(), OmiValue::Null);
            assert_eq!(serde_json::from_str::<OmiValue>("true").unwrap(), OmiValue::Bool(true));
            assert_eq!(serde_json::from_str::<OmiValue>("3.14").unwrap(), OmiValue::Number(3.14));
            assert_eq!(serde_json::from_str::<OmiValue>("\"hi\"").unwrap(), OmiValue::Str("hi".into()));
        }
    }
}
