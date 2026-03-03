use std::sync::{Mutex, MutexGuard};

pub fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|e| {
        log::error!("CRITICAL: {} mutex poisoned, recovering", name);
        e.into_inner()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn lock_normal() {
        let m = Mutex::new(42);
        let guard = lock_or_recover(&m, "test");
        assert_eq!(*guard, 42);
    }

    #[test]
    fn lock_recovers_from_poisoned() {
        let m = Arc::new(Mutex::new(42));
        // Poison the mutex by panicking while holding the lock
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("intentional poison");
        })
        .join();
        assert!(m.lock().is_err(), "mutex should be poisoned");
        let guard = lock_or_recover(&m, "test");
        assert_eq!(*guard, 42);
    }
}
