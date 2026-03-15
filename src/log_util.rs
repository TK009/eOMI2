use std::collections::HashMap;
use std::time::Instant;

const DEFAULT_WINDOW_SECS: u64 = 10;

struct Entry {
    last_emit: Instant,
    suppressed_count: u32,
}

pub struct RateLimiter {
    window_secs: u64,
    entries: HashMap<u64, Entry>,
}

fn hash_msg(msg: &str) -> u64 {
    // FNV-1a: simple, no extra deps, good enough for log dedup
    let mut h: u64 = 0xcbf29ce484222325;
    for b in msg.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            window_secs: DEFAULT_WINDOW_SECS,
            entries: HashMap::new(),
        }
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_window(window_secs: u64) -> Self {
        Self {
            window_secs,
            entries: HashMap::new(),
        }
    }

    /// Returns `true` if the message should be emitted now.
    ///
    /// When a previously-suppressed message becomes eligible again, a summary
    /// line is logged before returning `true`.
    pub fn should_emit(&mut self, msg: &str) -> bool {
        let key = hash_msg(msg);
        let now = Instant::now();

        match self.entries.get_mut(&key) {
            Some(entry) => {
                let elapsed = now.duration_since(entry.last_emit).as_secs();
                if elapsed >= self.window_secs {
                    let suppressed = entry.suppressed_count;
                    entry.last_emit = now;
                    entry.suppressed_count = 0;
                    if suppressed > 0 {
                        log::info!(
                            "(rate-limiter) suppressed {} repeated message(s) in last {}s",
                            suppressed,
                            elapsed
                        );
                    }
                    true
                } else {
                    entry.suppressed_count += 1;
                    false
                }
            }
            None => {
                self.entries.insert(
                    key,
                    Entry {
                        last_emit: now,
                        suppressed_count: 0,
                    },
                );
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn first_message_always_emits() {
        let mut rl = RateLimiter::new();
        assert!(rl.should_emit("hello"));
    }

    #[test]
    fn duplicate_within_window_is_suppressed() {
        let mut rl = RateLimiter::new();
        assert!(rl.should_emit("hello"));
        assert!(!rl.should_emit("hello"));
        assert!(!rl.should_emit("hello"));
    }

    #[test]
    fn different_messages_are_independent() {
        let mut rl = RateLimiter::new();
        assert!(rl.should_emit("aaa"));
        assert!(rl.should_emit("bbb"));
        assert!(!rl.should_emit("aaa"));
        assert!(!rl.should_emit("bbb"));
    }

    #[test]
    fn emits_again_after_window_expires() {
        let mut rl = RateLimiter::with_window(1);
        assert!(rl.should_emit("msg"));
        assert!(!rl.should_emit("msg"));
        thread::sleep(Duration::from_millis(1100));
        assert!(rl.should_emit("msg"));
    }

    #[test]
    fn suppressed_count_resets_after_emit() {
        let mut rl = RateLimiter::with_window(1);
        assert!(rl.should_emit("msg"));
        assert!(!rl.should_emit("msg"));
        assert!(!rl.should_emit("msg"));
        thread::sleep(Duration::from_millis(1100));
        // After window, should_emit returns true and resets counter
        assert!(rl.should_emit("msg"));
        // Immediately after, it's suppressed again (count is 0 -> 1)
        assert!(!rl.should_emit("msg"));
    }

    #[test]
    fn hash_is_deterministic() {
        assert_eq!(hash_msg("test"), hash_msg("test"));
        assert_ne!(hash_msg("test"), hash_msg("other"));
    }

    #[test]
    fn default_window_is_10s() {
        let rl = RateLimiter::new();
        assert_eq!(rl.window_secs, DEFAULT_WINDOW_SECS);
        assert_eq!(DEFAULT_WINDOW_SECS, 10);
    }
}
