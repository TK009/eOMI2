pub mod value;
pub mod item;
pub mod object;
pub mod tree;

/// OMI-Lite value type. Covers the common JSON primitives.
/// The spec allows "any JSON type" for `v`, but in practice
/// IoT values are numbers, strings, or booleans.
#[derive(Debug, Clone, PartialEq)]
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

}
