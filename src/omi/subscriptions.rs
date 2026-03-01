use std::collections::BTreeMap;
use std::collections::VecDeque;

use crate::odf::Value;

/// How a subscription delivers results.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryTarget {
    /// Push to a remote URL.
    Callback(String),
    /// Client polls via rid.
    Poll,
}

/// A single subscription entry.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub rid: String,
    pub path: String,
    /// -1.0 = event-based, >0 = interval in seconds.
    pub interval: f64,
    pub target: DeliveryTarget,
    /// Unix timestamp when the subscription was created.
    pub created_at: f64,
    /// Lifetime in seconds. -1.0 = never expires.
    pub ttl: f64,
    /// Next fire time (interval subscriptions only).
    pub trigger_time: f64,
}

impl Subscription {
    /// Returns true if this subscription has expired at the given time.
    fn is_expired(&self, now: f64) -> bool {
        if self.ttl < 0.0 {
            return false;
        }
        now > self.created_at + self.ttl
    }

    /// Returns the expiry timestamp, or `f64::INFINITY` if it never expires.
    fn expiry(&self) -> f64 {
        if self.ttl < 0.0 {
            f64::INFINITY
        } else {
            self.created_at + self.ttl
        }
    }
}

/// Result of polling a subscription.
#[derive(Debug)]
pub enum PollResult {
    /// Subscription found, buffer drained.
    Ok { path: String, values: Vec<Value> },
    /// Subscription exists but is not a poll subscription (callback-based).
    NotPollable,
    /// Subscription not found (never existed or already expired/cancelled).
    NotFound,
}

/// A pending delivery for the engine/transport layer to send.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub rid: String,
    pub path: String,
    pub values: Vec<Value>,
    pub target: DeliveryTarget,
}

/// Simple poll buffer — accumulates values, drained completely on each poll.
/// Drops oldest on overflow to bound memory. Uses VecDeque for O(1) front removal.
#[derive(Debug, Clone)]
pub struct PollBuffer {
    buf: VecDeque<Value>,
    capacity: usize,
}

impl PollBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            capacity,
        }
    }

    pub fn push(&mut self, values: &[Value]) {
        for v in values {
            if self.buf.len() >= self.capacity {
                self.buf.pop_front();
            }
            self.buf.push_back(v.clone());
        }
    }

    /// Drain all buffered values, returning them and leaving the buffer empty.
    pub fn drain(&mut self) -> Vec<Value> {
        self.buf.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

/// Maximum number of concurrent subscriptions.
pub const MAX_SUBSCRIPTIONS: usize = 32;
/// Default capacity for poll buffers.
pub const DEFAULT_POLL_CAPACITY: usize = 50;

/// Registry managing all active subscriptions.
pub struct SubscriptionRegistry {
    /// rid -> Subscription
    subscriptions: BTreeMap<String, Subscription>,
    /// path -> [rids] for event-based subscriptions only
    event_index: BTreeMap<String, Vec<String>>,
    /// rids sorted by trigger_time ascending (interval subs only)
    interval_queue: VecDeque<String>,
    /// rid -> PollBuffer for poll subscriptions only
    poll_buffers: BTreeMap<String, PollBuffer>,
    /// Monotonic counter for generating rids
    next_id: u32,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            subscriptions: BTreeMap::new(),
            event_index: BTreeMap::new(),
            interval_queue: VecDeque::new(),
            poll_buffers: BTreeMap::new(),
            next_id: 1,
        }
    }

    fn generate_rid(&mut self) -> String {
        let rid = format!("sub-{:03}", self.next_id);
        self.next_id += 1;
        rid
    }

    /// Create a new subscription.
    ///
    /// - `interval`: -1.0 for event-based, >0 for interval-based
    /// - `callback`: Some(url) for callback delivery, None for poll
    /// - `ttl`: lifetime in seconds, -1.0 for never expires
    /// - `now`: current unix timestamp
    pub fn create(
        &mut self,
        path: &str,
        interval: f64,
        callback: Option<&str>,
        ttl: f64,
        now: f64,
    ) -> Result<String, &'static str> {
        if interval != -1.0 {
            if interval <= 0.0 {
                return Err("interval must be -1 or positive");
            }
            if interval < 0.1 {
                return Err("interval must be >= 0.1 seconds");
            }
        }

        // Enforce limit
        if self.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err("subscription limit reached");
        }

        let rid = self.generate_rid();
        let target = match callback {
            Some(url) => DeliveryTarget::Callback(url.to_string()),
            None => DeliveryTarget::Poll,
        };

        let trigger_time = if interval > 0.0 {
            now + interval
        } else {
            0.0 // unused for event subs
        };

        let sub = Subscription {
            rid: rid.clone(),
            path: path.to_string(),
            interval,
            target: target.clone(),
            created_at: now,
            ttl,
            trigger_time,
        };

        self.subscriptions.insert(rid.clone(), sub);

        if interval < 0.0 {
            // Event-based subscription
            self.event_index
                .entry(path.to_string())
                .or_default()
                .push(rid.clone());
        } else {
            // Interval-based subscription — insert in sorted position
            self.insert_interval_sorted(rid.clone());
        }

        // Create poll buffer if needed
        if target == DeliveryTarget::Poll {
            self.poll_buffers
                .insert(rid.clone(), PollBuffer::new(DEFAULT_POLL_CAPACITY));
        }

        Ok(rid)
    }

    /// Cancel one or more subscriptions by rid. Unknown rids are silently skipped.
    /// Returns the number of subscriptions actually removed.
    pub fn cancel(&mut self, rids: &[String]) -> usize {
        let mut removed = 0;
        for rid in rids {
            if let Some(sub) = self.subscriptions.remove(rid) {
                removed += 1;

                if sub.interval < 0.0 {
                    // Remove from event index
                    if let Some(list) = self.event_index.get_mut(&sub.path) {
                        list.retain(|r| r != rid);
                        if list.is_empty() {
                            self.event_index.remove(&sub.path);
                        }
                    }
                } else {
                    // Remove from interval queue
                    self.interval_queue.retain(|r| r != rid);
                }

                self.poll_buffers.remove(rid);
            }
        }
        removed
    }

    /// Notify all event-based subscriptions watching the given path.
    ///
    /// - Callback subs produce a `Delivery` in the returned vec.
    /// - Poll subs buffer the values internally.
    /// - Expired subs are cleaned up during iteration.
    pub fn notify_event(&mut self, path: &str, values: &[Value], now: f64) -> Vec<Delivery> {
        let rids = match self.event_index.get(path) {
            Some(list) => list.clone(),
            None => return Vec::new(),
        };

        let mut deliveries = Vec::new();
        let mut expired = Vec::new();

        for rid in &rids {
            let sub = match self.subscriptions.get(rid) {
                Some(s) => s,
                None => continue,
            };

            if sub.is_expired(now) {
                expired.push(rid.clone());
                continue;
            }

            match &sub.target {
                DeliveryTarget::Callback(_) => {
                    deliveries.push(Delivery {
                        rid: rid.clone(),
                        path: path.to_string(),
                        values: values.to_vec(),
                        target: sub.target.clone(),
                    });
                }
                DeliveryTarget::Poll => {
                    if let Some(buf) = self.poll_buffers.get_mut(rid) {
                        buf.push(values);
                    }
                }
            }
        }

        // Clean up expired subscriptions
        if !expired.is_empty() {
            self.cancel(&expired);
        }

        deliveries
    }

    /// Insert a rid into the interval queue in sorted order by trigger_time.
    /// O(n) linear scan is acceptable given MAX_SUBSCRIPTIONS=32.
    fn insert_interval_sorted(&mut self, rid: String) {
        let trigger_time = self
            .subscriptions
            .get(&rid)
            .map(|s| s.trigger_time)
            .unwrap_or(f64::MAX);

        let pos = self
            .interval_queue
            .iter()
            .position(|r| {
                self.subscriptions
                    .get(r)
                    .map(|s| s.trigger_time)
                    .unwrap_or(f64::MAX)
                    > trigger_time
            })
            .unwrap_or(self.interval_queue.len());

        self.interval_queue.insert(pos, rid);
    }

    /// Process due interval subscriptions (adapted from thesis Algorithm 1).
    ///
    /// - `now`: current unix timestamp
    /// - `read_current`: closure that reads the current value at a path
    ///
    /// Returns `(deliveries, next_trigger_time)`.
    pub fn tick_intervals(
        &mut self,
        now: f64,
        read_current: &dyn Fn(&str) -> Option<Vec<Value>>,
    ) -> (Vec<Delivery>, Option<f64>) {
        let mut deliveries = Vec::new();
        let mut expired = Vec::new();

        loop {
            // Peek at front of queue
            let rid = match self.interval_queue.front() {
                Some(r) => r.clone(),
                None => break,
            };

            let trigger_time = match self.subscriptions.get(&rid) {
                Some(sub) => sub.trigger_time,
                None => {
                    // Stale entry, remove and continue
                    self.interval_queue.pop_front();
                    continue;
                }
            };

            if trigger_time > now {
                // Not yet due — this is the next trigger time
                break;
            }

            // Pop from front
            self.interval_queue.pop_front();

            let sub = match self.subscriptions.get(&rid) {
                Some(s) => s.clone(),
                None => continue,
            };

            // Check expiry
            if sub.is_expired(now) {
                expired.push(rid);
                continue;
            }

            // Read current value
            let values = read_current(&sub.path).unwrap_or_default();

            match &sub.target {
                DeliveryTarget::Callback(_) => {
                    deliveries.push(Delivery {
                        rid: rid.clone(),
                        path: sub.path.clone(),
                        values,
                        target: sub.target.clone(),
                    });
                }
                DeliveryTarget::Poll => {
                    if let Some(buf) = self.poll_buffers.get_mut(&rid) {
                        buf.push(&values);
                    }
                }
            }

            // Reschedule
            let next_trigger = sub.trigger_time + sub.interval;
            if next_trigger <= sub.expiry() {
                if let Some(s) = self.subscriptions.get_mut(&rid) {
                    s.trigger_time = next_trigger;
                }
                self.insert_interval_sorted(rid);
            } else {
                // Will expire before next trigger — remove
                expired.push(rid);
            }
        }

        // Clean up expired
        if !expired.is_empty() {
            self.cancel(&expired);
        }

        let next = self
            .interval_queue
            .front()
            .and_then(|rid| self.subscriptions.get(rid))
            .map(|s| s.trigger_time);

        (deliveries, next)
    }

    /// Poll a subscription's buffered values. Drains the buffer on success.
    pub fn poll(&mut self, rid: &str, now: f64) -> PollResult {
        let sub = match self.subscriptions.get(rid) {
            Some(s) => s,
            None => return PollResult::NotFound,
        };

        if sub.is_expired(now) {
            let rid_owned = rid.to_string();
            self.cancel(&[rid_owned]);
            return PollResult::NotFound;
        }

        let path = sub.path.clone();
        match self.poll_buffers.get_mut(rid) {
            Some(buf) => PollResult::Ok { path, values: buf.drain() },
            None => PollResult::NotPollable,
        }
    }

    /// Full sweep: remove all expired subscriptions. Returns the number removed.
    pub fn expire(&mut self, now: f64) -> usize {
        let expired: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, sub)| sub.is_expired(now))
            .map(|(rid, _)| rid.clone())
            .collect();
        let count = expired.len();
        if count > 0 {
            self.cancel(&expired);
        }
        count
    }

    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }

    pub fn contains(&self, rid: &str) -> bool {
        self.subscriptions.contains_key(rid)
    }

    /// Returns the next interval trigger time, if any interval subs exist.
    pub fn next_trigger_time(&self) -> Option<f64> {
        self.interval_queue
            .front()
            .and_then(|rid| self.subscriptions.get(rid))
            .map(|s| s.trigger_time)
    }
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odf::OmiValue;

    fn val(n: f64, t: f64) -> Value {
        Value::new(OmiValue::Number(n), Some(t))
    }

    // --- PollBuffer tests ---

    #[test]
    fn poll_buffer_push_and_drain() {
        let mut buf = PollBuffer::new(10);
        buf.push(&[val(1.0, 100.0), val(2.0, 200.0)]);
        assert_eq!(buf.len(), 2);
        assert!(!buf.is_empty());

        let drained = buf.drain();
        assert_eq!(drained.len(), 2);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn poll_buffer_overflow_drops_oldest() {
        let mut buf = PollBuffer::new(3);
        buf.push(&[val(1.0, 100.0), val(2.0, 200.0), val(3.0, 300.0)]);
        buf.push(&[val(4.0, 400.0)]);
        assert_eq!(buf.len(), 3);

        let drained = buf.drain();
        // Should have 2, 3, 4 (oldest=1 was dropped)
        assert_eq!(drained[0].v, OmiValue::Number(2.0));
        assert_eq!(drained[1].v, OmiValue::Number(3.0));
        assert_eq!(drained[2].v, OmiValue::Number(4.0));
    }

    #[test]
    fn poll_buffer_empty_drain() {
        let mut buf = PollBuffer::new(10);
        let drained = buf.drain();
        assert!(drained.is_empty());
    }

    // --- RID generation ---

    #[test]
    fn rid_sequential_increment() {
        let mut reg = SubscriptionRegistry::new();
        let r1 = reg.generate_rid();
        let r2 = reg.generate_rid();
        let r3 = reg.generate_rid();
        assert_eq!(r1, "sub-001");
        assert_eq!(r2, "sub-002");
        assert_eq!(r3, "sub-003");
    }

    // --- Create ---

    #[test]
    fn create_event_subscription() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, Some("http://cb.example.com"), 60.0, 1000.0).unwrap();
        assert!(rid.starts_with("sub-"));
        assert_eq!(reg.len(), 1);
        assert!(reg.contains(&rid));

        // Should be in event index
        let rids = reg.event_index.get("/A/Temp").unwrap();
        assert!(rids.contains(&rid));
        // Should not be in interval queue
        assert!(reg.interval_queue.is_empty());
    }

    #[test]
    fn create_interval_subscription() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", 5.0, Some("http://cb.example.com"), 60.0, 1000.0).unwrap();
        assert_eq!(reg.len(), 1);

        // Should be in interval queue, not event index
        assert!(reg.event_index.is_empty());
        assert_eq!(reg.interval_queue.len(), 1);
        assert_eq!(reg.interval_queue[0], rid);

        // Trigger time should be now + interval
        let sub = reg.subscriptions.get(&rid).unwrap();
        assert_eq!(sub.trigger_time, 1005.0);
    }

    #[test]
    fn create_poll_subscription() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 60.0, 1000.0).unwrap();
        assert_eq!(reg.len(), 1);

        // Should have a poll buffer
        assert!(reg.poll_buffers.contains_key(&rid));
    }

    #[test]
    fn create_reject_zero_interval() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg.create("/A/Temp", 0.0, None, 60.0, 1000.0).unwrap_err();
        assert_eq!(err, "interval must be -1 or positive");
    }

    #[test]
    fn create_reject_negative_interval() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg.create("/A/Temp", -2.0, None, 60.0, 1000.0).unwrap_err();
        assert_eq!(err, "interval must be -1 or positive");
    }

    #[test]
    fn create_reject_too_small_interval() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg.create("/A/Temp", 0.05, None, 60.0, 1000.0).unwrap_err();
        assert_eq!(err, "interval must be >= 0.1 seconds");
    }

    #[test]
    fn create_reject_fractional_negative() {
        let mut reg = SubscriptionRegistry::new();
        let err = reg.create("/A/Temp", -0.5, None, 60.0, 1000.0).unwrap_err();
        assert_eq!(err, "interval must be -1 or positive");
    }

    #[test]
    fn create_enforce_limit() {
        let mut reg = SubscriptionRegistry::new();
        for i in 0..MAX_SUBSCRIPTIONS {
            reg.create(
                &format!("/path/{}", i),
                -1.0,
                Some("http://cb"),
                -1.0,
                1000.0,
            )
            .unwrap();
        }
        let err = reg.create("/overflow", -1.0, Some("http://cb"), -1.0, 1000.0).unwrap_err();
        assert_eq!(err, "subscription limit reached");
        assert_eq!(reg.len(), MAX_SUBSCRIPTIONS);
    }

    #[test]
    fn create_multiple_on_same_path() {
        let mut reg = SubscriptionRegistry::new();
        let r1 = reg.create("/A/Temp", -1.0, Some("http://cb1"), 60.0, 1000.0).unwrap();
        let r2 = reg.create("/A/Temp", -1.0, Some("http://cb2"), 60.0, 1000.0).unwrap();
        assert_ne!(r1, r2);
        assert_eq!(reg.len(), 2);

        let rids = reg.event_index.get("/A/Temp").unwrap();
        assert_eq!(rids.len(), 2);
    }

    // --- Cancel ---

    #[test]
    fn cancel_single() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let removed = reg.cancel(&[rid.clone()]);
        assert_eq!(removed, 1);
        assert_eq!(reg.len(), 0);
        assert!(!reg.contains(&rid));
    }

    #[test]
    fn cancel_multi() {
        let mut reg = SubscriptionRegistry::new();
        let r1 = reg.create("/A", -1.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let r2 = reg.create("/B", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let r3 = reg.create("/C", -1.0, None, 60.0, 1000.0).unwrap();
        assert_eq!(reg.len(), 3);

        let removed = reg.cancel(&[r1, r2, r3]);
        assert_eq!(removed, 3);
        assert_eq!(reg.len(), 0);
        assert!(reg.event_index.is_empty());
        assert!(reg.interval_queue.is_empty());
        assert!(reg.poll_buffers.is_empty());
    }

    #[test]
    fn cancel_unknown_rid_idempotent() {
        let mut reg = SubscriptionRegistry::new();
        let removed = reg.cancel(&["nonexistent".to_string()]);
        assert_eq!(removed, 0);
    }

    #[test]
    fn cancel_removes_event_index_and_poll_buffer() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 60.0, 1000.0).unwrap();
        assert!(reg.event_index.contains_key("/A/Temp"));
        assert!(reg.poll_buffers.contains_key(&rid));

        reg.cancel(&[rid]);
        assert!(!reg.event_index.contains_key("/A/Temp"));
        assert!(reg.poll_buffers.is_empty());
    }

    #[test]
    fn cancel_removes_interval_queue_entry() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        assert_eq!(reg.interval_queue.len(), 1);

        reg.cancel(&[rid]);
        assert!(reg.interval_queue.is_empty());
    }

    // --- Event notify ---

    #[test]
    fn notify_event_callback_delivery() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        let values = vec![val(22.5, 1010.0)];
        let deliveries = reg.notify_event("/A/Temp", &values, 1010.0);
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].rid, rid);
        assert_eq!(deliveries[0].path, "/A/Temp");
        assert_eq!(deliveries[0].values.len(), 1);
        assert_eq!(deliveries[0].target, DeliveryTarget::Callback("http://cb".into()));
    }

    #[test]
    fn notify_event_poll_buffering() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 60.0, 1000.0).unwrap();

        let values = vec![val(22.5, 1010.0)];
        let deliveries = reg.notify_event("/A/Temp", &values, 1010.0);
        // Poll subs don't produce deliveries
        assert!(deliveries.is_empty());

        // Values should be buffered
        let buf = reg.poll_buffers.get(&rid).unwrap();
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn notify_event_no_subscribers() {
        let mut reg = SubscriptionRegistry::new();
        let deliveries = reg.notify_event("/NoSubs", &[val(1.0, 100.0)], 100.0);
        assert!(deliveries.is_empty());
    }

    #[test]
    fn notify_event_expired_cleanup() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, Some("http://cb"), 10.0, 1000.0).unwrap();

        // Notify after TTL expired (created_at=1000, ttl=10, now=1020 > 1010)
        let deliveries = reg.notify_event("/A/Temp", &[val(1.0, 1020.0)], 1020.0);
        assert!(deliveries.is_empty());
        assert!(!reg.contains(&rid));
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn notify_event_ttl_forever() {
        let mut reg = SubscriptionRegistry::new();
        let _rid = reg.create("/A/Temp", -1.0, Some("http://cb"), -1.0, 1000.0).unwrap();

        // Should still deliver even at very large timestamps
        let deliveries = reg.notify_event("/A/Temp", &[val(1.0, 999999.0)], 999999.0);
        assert_eq!(deliveries.len(), 1);
    }

    // --- Interval tick ---

    #[test]
    fn tick_single_due() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        // Trigger at 1005, tick at 1006
        let (deliveries, next) = reg.tick_intervals(1006.0, &|_path| {
            Some(vec![val(22.5, 1006.0)])
        });
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].rid, rid);
        assert_eq!(deliveries[0].values.len(), 1);

        // Should be rescheduled: 1005 + 5 = 1010
        assert_eq!(next, Some(1010.0));
    }

    #[test]
    fn tick_not_yet_due() {
        let mut reg = SubscriptionRegistry::new();
        let _rid = reg.create("/A/Temp", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        // Trigger at 1005, tick at 1003 — not due yet
        let (deliveries, next) = reg.tick_intervals(1003.0, &|_| Some(vec![]));
        assert!(deliveries.is_empty());
        assert_eq!(next, Some(1005.0));
    }

    #[test]
    fn tick_reschedule() {
        let mut reg = SubscriptionRegistry::new();
        let _rid = reg.create("/A/Temp", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        // First tick at 1006 (due at 1005)
        let (_, next) = reg.tick_intervals(1006.0, &|_| Some(vec![val(1.0, 1006.0)]));
        assert_eq!(next, Some(1010.0));

        // Second tick at 1011 (due at 1010)
        let (deliveries, next) = reg.tick_intervals(1011.0, &|_| Some(vec![val(2.0, 1011.0)]));
        assert_eq!(deliveries.len(), 1);
        assert_eq!(next, Some(1015.0));
    }

    #[test]
    fn tick_expiry_on_reschedule() {
        let mut reg = SubscriptionRegistry::new();
        // TTL=12 means expires at 1012. Interval=5, first trigger at 1005, reschedule to 1010.
        // Next would be 1015 > 1012, so subscription should be removed.
        let rid = reg.create("/A/Temp", 5.0, Some("http://cb"), 12.0, 1000.0).unwrap();

        // First tick at 1006
        let (deliveries, _) = reg.tick_intervals(1006.0, &|_| Some(vec![val(1.0, 1006.0)]));
        assert_eq!(deliveries.len(), 1);
        assert!(reg.contains(&rid)); // rescheduled to 1010

        // Second tick at 1011
        let (deliveries, _) = reg.tick_intervals(1011.0, &|_| Some(vec![val(2.0, 1011.0)]));
        assert_eq!(deliveries.len(), 1);
        // Next trigger would be 1015 > 1012 (expiry), so sub should be removed
        assert!(!reg.contains(&rid));
    }

    #[test]
    fn tick_multiple_due() {
        let mut reg = SubscriptionRegistry::new();
        let r1 = reg.create("/A", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let r2 = reg.create("/B", 3.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        // r2 triggers at 1003, r1 at 1005. Tick at 1006:
        //   - r2 fires (trigger 1003), rescheduled to 1006
        //   - r1 fires (trigger 1005), rescheduled to 1010
        //   - r2 fires again (trigger 1006 <= 1006), rescheduled to 1009
        let (deliveries, _next) = reg.tick_intervals(1006.0, &|_| Some(vec![val(1.0, 1006.0)]));
        assert_eq!(deliveries.len(), 3);
        assert_eq!(deliveries[0].rid, r2);
        assert_eq!(deliveries[1].rid, r1);
        assert_eq!(deliveries[2].rid, r2);
    }

    #[test]
    fn tick_ordering_preserved() {
        let mut reg = SubscriptionRegistry::new();
        let _r1 = reg.create("/A", 10.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let _r2 = reg.create("/B", 3.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let _r3 = reg.create("/C", 7.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        // Queue should be sorted: r2(1003), r3(1007), r1(1010)
        assert_eq!(reg.next_trigger_time(), Some(1003.0));
    }

    #[test]
    fn tick_poll_buffering() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", 5.0, None, 60.0, 1000.0).unwrap();

        let (deliveries, _) = reg.tick_intervals(1006.0, &|_| Some(vec![val(22.5, 1006.0)]));
        // Poll subs don't produce deliveries
        assert!(deliveries.is_empty());

        // Should be buffered
        let buf = reg.poll_buffers.get(&rid).unwrap();
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn tick_empty_queue() {
        let mut reg = SubscriptionRegistry::new();
        let (deliveries, next) = reg.tick_intervals(1000.0, &|_| None);
        assert!(deliveries.is_empty());
        assert_eq!(next, None);
    }

    // --- Poll ---

    #[test]
    fn poll_buffered_retrieval() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 60.0, 1000.0).unwrap();

        reg.notify_event("/A/Temp", &[val(22.5, 1010.0), val(23.0, 1011.0)], 1011.0);
        match reg.poll(&rid, 1012.0) {
            PollResult::Ok { path, values } => {
                assert_eq!(path, "/A/Temp");
                assert_eq!(values.len(), 2);
            }
            other => panic!("expected PollResult::Ok, got {:?}", other),
        }
    }

    #[test]
    fn poll_unknown_rid() {
        let mut reg = SubscriptionRegistry::new();
        assert!(matches!(reg.poll("nonexistent", 1000.0), PollResult::NotFound));
    }

    #[test]
    fn poll_expired_sub() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 10.0, 1000.0).unwrap();
        reg.notify_event("/A/Temp", &[val(1.0, 1005.0)], 1005.0);

        // Poll after expiry
        assert!(matches!(reg.poll(&rid, 1020.0), PollResult::NotFound));
        assert!(!reg.contains(&rid));
    }

    #[test]
    fn poll_consecutive_drains() {
        let mut reg = SubscriptionRegistry::new();
        let rid = reg.create("/A/Temp", -1.0, None, 60.0, 1000.0).unwrap();

        reg.notify_event("/A/Temp", &[val(1.0, 1001.0)], 1001.0);
        match reg.poll(&rid, 1002.0) {
            PollResult::Ok { values, .. } => assert_eq!(values.len(), 1),
            other => panic!("expected Ok, got {:?}", other),
        }

        // Second poll — buffer should be empty now
        match reg.poll(&rid, 1003.0) {
            PollResult::Ok { values, .. } => assert!(values.is_empty()),
            other => panic!("expected Ok, got {:?}", other),
        }

        // Notify again
        reg.notify_event("/A/Temp", &[val(2.0, 1004.0)], 1004.0);
        match reg.poll(&rid, 1005.0) {
            PollResult::Ok { values, .. } => {
                assert_eq!(values.len(), 1);
                assert_eq!(values[0].v, OmiValue::Number(2.0));
            }
            other => panic!("expected Ok, got {:?}", other),
        }
    }

    // --- Expire ---

    #[test]
    fn expire_removes_expired() {
        let mut reg = SubscriptionRegistry::new();
        reg.create("/A", -1.0, Some("http://cb"), 10.0, 1000.0).unwrap();
        reg.create("/B", -1.0, Some("http://cb"), 20.0, 1000.0).unwrap();

        // At t=1015, first sub expired (created 1000, ttl 10 → expires 1010)
        // second still active (expires 1020)
        let removed = reg.expire(1015.0);
        assert_eq!(removed, 1);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn expire_keeps_active() {
        let mut reg = SubscriptionRegistry::new();
        reg.create("/A", -1.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        let removed = reg.expire(1010.0);
        assert_eq!(removed, 0);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn expire_keeps_forever() {
        let mut reg = SubscriptionRegistry::new();
        reg.create("/A", -1.0, Some("http://cb"), -1.0, 1000.0).unwrap();
        let removed = reg.expire(999999.0);
        assert_eq!(removed, 0);
        assert_eq!(reg.len(), 1);
    }

    // --- Integration ---

    #[test]
    fn full_event_lifecycle() {
        let mut reg = SubscriptionRegistry::new();

        // Create event poll subscription
        let rid = reg.create("/Sensor/Temp", -1.0, None, 60.0, 1000.0).unwrap();
        assert_eq!(reg.len(), 1);

        // Notify event
        reg.notify_event("/Sensor/Temp", &[val(22.5, 1010.0)], 1010.0);

        // Poll — get the value
        match reg.poll(&rid, 1011.0) {
            PollResult::Ok { path, values } => {
                assert_eq!(path, "/Sensor/Temp");
                assert_eq!(values.len(), 1);
                assert_eq!(values[0].v, OmiValue::Number(22.5));
            }
            other => panic!("expected Ok, got {:?}", other),
        }

        // Poll again — empty
        match reg.poll(&rid, 1012.0) {
            PollResult::Ok { values, .. } => assert!(values.is_empty()),
            other => panic!("expected Ok, got {:?}", other),
        }

        // Cancel
        let removed = reg.cancel(&[rid.clone()]);
        assert_eq!(removed, 1);
        assert!(reg.is_empty());
    }

    #[test]
    fn full_interval_lifecycle() {
        let mut reg = SubscriptionRegistry::new();

        // Create interval callback subscription, TTL=20
        let rid = reg.create("/Sensor/Temp", 5.0, Some("http://cb"), 20.0, 1000.0).unwrap();

        // Tick at 1006 — first delivery
        let (deliveries, next) = reg.tick_intervals(1006.0, &|_| {
            Some(vec![val(22.5, 1006.0)])
        });
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].rid, rid);
        assert_eq!(next, Some(1010.0));

        // Tick at 1011 — second delivery
        let (deliveries, next) = reg.tick_intervals(1011.0, &|_| {
            Some(vec![val(23.0, 1011.0)])
        });
        assert_eq!(deliveries.len(), 1);
        assert_eq!(next, Some(1015.0));

        // Tick at 1016 — third delivery, next would be 1020 = expiry
        let (deliveries, _) = reg.tick_intervals(1016.0, &|_| {
            Some(vec![val(24.0, 1016.0)])
        });
        assert_eq!(deliveries.len(), 1);
        // 1015 + 5 = 1020 <= expiry 1020, so rescheduled
        assert!(reg.contains(&rid));

        // Tick at 1020 — triggers at 1020, delivers (still within TTL since now==expiry)
        // Next would be 1025 > 1020 (expiry), so sub is removed
        let (deliveries, next) = reg.tick_intervals(1020.0, &|_| {
            Some(vec![val(25.0, 1020.0)])
        });
        assert_eq!(deliveries.len(), 1);
        assert!(!reg.contains(&rid));
        assert_eq!(next, None);
    }

    #[test]
    fn mixed_event_and_interval() {
        let mut reg = SubscriptionRegistry::new();

        // Event sub (poll)
        let event_rid = reg.create("/Sensor/Temp", -1.0, None, 60.0, 1000.0).unwrap();
        // Interval sub (callback)
        let interval_rid = reg.create("/Sensor/Temp", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();

        assert_eq!(reg.len(), 2);

        // Event notification
        reg.notify_event("/Sensor/Temp", &[val(22.5, 1002.0)], 1002.0);

        // Poll the event sub
        match reg.poll(&event_rid, 1003.0) {
            PollResult::Ok { values, .. } => assert_eq!(values.len(), 1),
            other => panic!("expected Ok, got {:?}", other),
        }

        // Tick the interval sub
        let (deliveries, _) = reg.tick_intervals(1006.0, &|_| {
            Some(vec![val(23.0, 1006.0)])
        });
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].rid, interval_rid);

        // Both still active
        assert!(reg.contains(&event_rid));
        assert!(reg.contains(&interval_rid));

        // Cancel both
        reg.cancel(&[event_rid.clone(), interval_rid.clone()]);
        assert!(reg.is_empty());
    }

    // --- Query methods ---

    #[test]
    fn query_methods() {
        let mut reg = SubscriptionRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.next_trigger_time(), None);

        let rid = reg.create("/A", 5.0, Some("http://cb"), 60.0, 1000.0).unwrap();
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
        assert!(reg.contains(&rid));
        assert!(!reg.contains("other"));
        assert_eq!(reg.next_trigger_time(), Some(1005.0));
    }
}
