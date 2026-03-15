#[cfg(feature = "json")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "json")]
use serde::ser::SerializeSeq;

use super::OmiValue;
use crate::psram::PsramBox;

/// A timestamped value in the OMI data model.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json", derive(Serialize, Deserialize))]
pub struct Value {
    pub v: OmiValue,
    #[cfg_attr(feature = "json", serde(skip_serializing_if = "Option::is_none"))]
    pub t: Option<f64>,
}

impl Value {
    pub fn new(v: OmiValue, t: Option<f64>) -> Self {
        Self { v, t }
    }

    /// Create a value with the current timestamp (seconds since UNIX epoch).
    #[cfg(feature = "std")]
    pub fn now(v: OmiValue) -> Self {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .ok();
        Self { v, t }
    }
}

/// Fixed-capacity circular buffer of `Value` entries.
///
/// Overwrites oldest entries when full. Provides efficient O(1) insertion
/// and query methods returning newest-first ordering as the OMI spec requires.
///
/// Backed by `PsramBox` so the buffer lands in PSRAM on ESP32.
#[derive(Debug, Clone, PartialEq)]
pub struct RingBuffer {
    buf: PsramBox<Value>,
    /// Index where the next push will write. When `len == capacity`, this
    /// also points to the oldest element (which will be overwritten).
    head: usize,
    len: usize,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "RingBuffer capacity must be > 0");
        Self {
            buf: PsramBox::new(capacity),
            head: 0,
            len: 0,
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Push a value, overwriting the oldest if at capacity.
    pub fn push(&mut self, value: Value) {
        if self.len < self.capacity {
            // Still growing — append
            self.buf.push(value);
            self.len += 1;
            self.head = self.len % self.capacity;
        } else {
            // Full — overwrite oldest
            self.buf.set(self.head, value);
            self.head = (self.head + 1) % self.capacity;
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.head = 0;
        self.len = 0;
    }

    /// Index of the oldest element. Only valid when `len > 0`.
    fn oldest_index(&self) -> usize {
        if self.len < self.capacity { 0 } else { self.head }
    }

    /// Get element by logical index (0 = oldest, len-1 = newest).
    fn get(&self, logical: usize) -> &Value {
        let idx = (self.oldest_index() + logical) % self.capacity;
        self.buf.get(idx)
    }

    /// Iterate from oldest to newest.
    fn iter_oldest_to_newest(&self) -> impl Iterator<Item = &Value> {
        let len = self.len;
        (0..len).map(move |i| self.get(i))
    }

    /// Return up to `n` newest values, newest first.
    pub fn newest(&self, n: usize) -> Vec<Value> {
        let take = n.min(self.len);
        (0..take)
            .map(|i| self.get(self.len - 1 - i).clone())
            .collect()
    }

    /// Return up to `n` oldest values, newest first (per spec ordering).
    pub fn oldest(&self, n: usize) -> Vec<Value> {
        let take = n.min(self.len);
        (0..take)
            .rev()
            .map(|i| self.get(i).clone())
            .collect()
    }

    /// Return values with timestamp in [begin, end], newest first.
    pub fn range(&self, begin: f64, end: f64) -> Vec<Value> {
        let mut result: Vec<Value> = self
            .iter_oldest_to_newest()
            .filter(|v| {
                v.t.is_some_and(|t| t >= begin && t <= end)
            })
            .cloned()
            .collect();
        result.reverse();
        result
    }

    /// Combined query: time filter first, then count limit, newest first.
    pub fn query(
        &self,
        newest: Option<usize>,
        oldest: Option<usize>,
        begin: Option<f64>,
        end: Option<f64>,
    ) -> Vec<Value> {
        // Collect all values oldest-to-newest
        let mut values: Vec<Value> = self.iter_oldest_to_newest().cloned().collect();

        // Apply time range filter
        if begin.is_some() || end.is_some() {
            values.retain(|v| {
                let t = match v.t {
                    Some(t) => t,
                    None => return false,
                };
                if let Some(b) = begin {
                    if t < b { return false; }
                }
                if let Some(e) = end {
                    if t > e { return false; }
                }
                true
            });
        }

        // Apply count limits (newest takes precedence over oldest)
        if let Some(n) = newest {
            let skip = values.len().saturating_sub(n);
            values = values[skip..].to_vec();
        } else if let Some(n) = oldest {
            values.truncate(n);
        }

        // Return newest first
        values.reverse();
        values
    }
}

#[cfg(feature = "json")]
impl Serialize for RingBuffer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len))?;
        // Serialize newest first (spec ordering)
        for i in (0..self.len).rev() {
            seq.serialize_element(self.get(i))?;
        }
        seq.end()
    }
}

#[cfg(feature = "json")]
impl<'de> Deserialize<'de> for RingBuffer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values: Vec<Value> = Vec::deserialize(deserializer)?;
        let capacity = values.len().max(1);
        let mut rb = RingBuffer::new(capacity);
        // Input is newest-first; push in reverse so oldest goes in first
        for v in values.into_iter().rev() {
            rb.push(v);
        }
        Ok(rb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(n: f64, t: f64) -> Value {
        Value::new(OmiValue::Number(n), Some(t))
    }

    fn val_no_t(n: f64) -> Value {
        Value::new(OmiValue::Number(n), None)
    }

    #[test]
    fn push_within_capacity() {
        let mut rb = RingBuffer::new(5);
        rb.push(val(1.0, 100.0));
        rb.push(val(2.0, 200.0));
        rb.push(val(3.0, 300.0));
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.capacity(), 5);
    }

    #[test]
    fn push_overflow_wraps() {
        let mut rb = RingBuffer::new(3);
        rb.push(val(1.0, 100.0));
        rb.push(val(2.0, 200.0));
        rb.push(val(3.0, 300.0));
        rb.push(val(4.0, 400.0)); // overwrites val(1.0)
        assert_eq!(rb.len(), 3);

        let newest = rb.newest(10);
        assert_eq!(newest.len(), 3);
        // Newest first: 4, 3, 2
        assert_eq!(newest[0].v, OmiValue::Number(4.0));
        assert_eq!(newest[1].v, OmiValue::Number(3.0));
        assert_eq!(newest[2].v, OmiValue::Number(2.0));
    }

    #[test]
    fn newest_query() {
        let mut rb = RingBuffer::new(5);
        for i in 1..=5 {
            rb.push(val(i as f64, i as f64 * 100.0));
        }
        let result = rb.newest(2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].v, OmiValue::Number(5.0));
        assert_eq!(result[1].v, OmiValue::Number(4.0));
    }

    #[test]
    fn oldest_query() {
        let mut rb = RingBuffer::new(5);
        for i in 1..=5 {
            rb.push(val(i as f64, i as f64 * 100.0));
        }
        let result = rb.oldest(2);
        assert_eq!(result.len(), 2);
        // oldest returns oldest values but in newest-first order
        assert_eq!(result[0].v, OmiValue::Number(2.0));
        assert_eq!(result[1].v, OmiValue::Number(1.0));
    }

    #[test]
    fn time_range_query() {
        let mut rb = RingBuffer::new(10);
        for i in 1..=5 {
            rb.push(val(i as f64, i as f64 * 100.0));
        }
        let result = rb.range(200.0, 400.0);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].v, OmiValue::Number(4.0));
        assert_eq!(result[1].v, OmiValue::Number(3.0));
        assert_eq!(result[2].v, OmiValue::Number(2.0));
    }

    #[test]
    fn combined_query_range_and_newest() {
        let mut rb = RingBuffer::new(10);
        for i in 1..=5 {
            rb.push(val(i as f64, i as f64 * 100.0));
        }
        // Range [200, 500], then newest 2
        let result = rb.query(Some(2), None, Some(200.0), Some(500.0));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].v, OmiValue::Number(5.0));
        assert_eq!(result[1].v, OmiValue::Number(4.0));
    }

    #[test]
    fn combined_query_range_and_oldest() {
        let mut rb = RingBuffer::new(10);
        for i in 1..=5 {
            rb.push(val(i as f64, i as f64 * 100.0));
        }
        // Range [100, 500], then oldest 2
        let result = rb.query(None, Some(2), Some(100.0), Some(500.0));
        assert_eq!(result.len(), 2);
        // oldest 2 values are 1,2 but returned newest-first: 2, 1
        assert_eq!(result[0].v, OmiValue::Number(2.0));
        assert_eq!(result[1].v, OmiValue::Number(1.0));
    }

    #[test]
    fn empty_buffer_queries() {
        let rb = RingBuffer::new(5);
        assert!(rb.is_empty());
        assert_eq!(rb.newest(5).len(), 0);
        assert_eq!(rb.oldest(5).len(), 0);
        assert_eq!(rb.range(0.0, 1000.0).len(), 0);
        assert_eq!(rb.query(Some(5), None, None, None).len(), 0);
    }

    #[test]
    fn clear() {
        let mut rb = RingBuffer::new(5);
        rb.push(val(1.0, 100.0));
        rb.push(val(2.0, 200.0));
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.newest(5).len(), 0);
    }

    #[test]
    fn values_without_timestamp_excluded_from_range() {
        let mut rb = RingBuffer::new(5);
        rb.push(val_no_t(1.0));
        rb.push(val(2.0, 200.0));
        let result = rb.range(0.0, 1000.0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].v, OmiValue::Number(2.0));
    }

    #[cfg(feature = "json")]
    mod json {
        use super::*;

        #[test]
        fn serde_roundtrip() {
            let mut rb = RingBuffer::new(5);
            rb.push(val(1.0, 100.0));
            rb.push(val(2.0, 200.0));
            rb.push(val(3.0, 300.0));

            let json = serde_json::to_string(&rb).unwrap();
            let rb2: RingBuffer = serde_json::from_str(&json).unwrap();

            assert_eq!(rb2.len(), 3);
            let newest = rb2.newest(3);
            assert_eq!(newest[0].v, OmiValue::Number(3.0));
            assert_eq!(newest[1].v, OmiValue::Number(2.0));
            assert_eq!(newest[2].v, OmiValue::Number(1.0));
        }

        #[test]
        fn serialize_newest_first() {
            let mut rb = RingBuffer::new(5);
            rb.push(val(1.0, 100.0));
            rb.push(val(2.0, 200.0));
            let json = serde_json::to_string(&rb).unwrap();
            let arr: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
            // First element should be newest
            assert_eq!(arr[0]["v"], 2.0);
            assert_eq!(arr[1]["v"], 1.0);
        }
    }

    #[test]
    fn newest_after_overflow() {
        let mut rb = RingBuffer::new(3);
        for i in 1..=10 {
            rb.push(val(i as f64, i as f64 * 10.0));
        }
        assert_eq!(rb.len(), 3);
        let newest = rb.newest(3);
        assert_eq!(newest[0].v, OmiValue::Number(10.0));
        assert_eq!(newest[1].v, OmiValue::Number(9.0));
        assert_eq!(newest[2].v, OmiValue::Number(8.0));
    }

    #[test]
    fn value_now_has_timestamp() {
        let v = Value::now(OmiValue::Number(42.0));
        assert!(v.t.is_some());
        assert!(v.t.unwrap() > 0.0);
    }
}
