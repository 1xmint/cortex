//! Mutex locking that survives a panic.
//!
//! `std::sync::Mutex` poisons when a thread panics while holding the guard, and
//! every later `lock().unwrap()` on that mutex panics in turn. For long-lived
//! process state — the database handle, rate-limit buckets, the circuit
//! breaker — that converts one bad request into a permanent, process-wide
//! outage. None of this state is left structurally broken by a panic: the worst
//! case is a partially updated in-memory counter, which is strictly better than
//! refusing every subsequent request.
//!
//! Prefer [`LockRecovering::lock_recovering`] over `lock().unwrap()` anywhere the
//! mutex outlives a single request.

use std::sync::{Mutex, MutexGuard};

pub(crate) trait LockRecovering<T> {
    /// Take the lock, recovering the guard if a previous holder panicked.
    fn lock_recovering(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecovering<T> for Mutex<T> {
    fn lock_recovering(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_poisoned_mutex_still_hands_out_its_value() {
        let counter = Arc::new(Mutex::new(1_u32));

        let poisoner = Arc::clone(&counter);
        let outcome = std::thread::spawn(move || {
            let mut guard = poisoner.lock_recovering();
            *guard = 2;
            panic!("simulated panic while holding the lock");
        })
        .join();
        assert!(outcome.is_err(), "the worker thread must have panicked");

        assert!(counter.lock().is_err(), "the mutex is genuinely poisoned");
        assert_eq!(
            *counter.lock_recovering(),
            2,
            "the value is still reachable"
        );
    }
}
