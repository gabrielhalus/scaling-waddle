use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

const MAX_KEYS: usize = 10_000;

pub struct FailureLimiter {
    max: usize,
    window: Duration,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl FailureLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn retry_after(&self, key: &str, now: Instant) -> Option<Duration> {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let recent = failures.get_mut(key)?;
        recent.retain(|at| now.duration_since(*at) < self.window);
        if recent.len() < self.max {
            return None;
        }
        let oldest = *recent.first()?;
        Some(self.window.saturating_sub(now.duration_since(oldest)))
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        if failures.len() >= MAX_KEYS && !failures.contains_key(key) {
            failures.retain(|_, recent| {
                recent.retain(|at| now.duration_since(*at) < self.window);
                !recent.is_empty()
            });
        }
        if failures.len() >= MAX_KEYS && !failures.contains_key(key) {
            return;
        }
        failures.entry(key.to_owned()).or_default().push(now);
    }

    pub fn clear(&self, key: &str) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> FailureLimiter {
        FailureLimiter::new(3, Duration::from_secs(60))
    }

    #[test]
    fn blocks_after_the_maximum_number_of_failures() {
        let (limiter, now) = (limiter(), Instant::now());
        for _ in 0..2 {
            limiter.record_failure("a", now);
        }
        assert!(limiter.retry_after("a", now).is_none());
        limiter.record_failure("a", now);
        assert_eq!(limiter.retry_after("a", now), Some(Duration::from_secs(60)));
    }

    #[test]
    fn keys_are_independent() {
        let (limiter, now) = (limiter(), Instant::now());
        for _ in 0..3 {
            limiter.record_failure("a", now);
        }
        assert!(limiter.retry_after("b", now).is_none());
    }

    #[test]
    fn failures_expire_with_the_window() {
        let (limiter, now) = (limiter(), Instant::now());
        for _ in 0..3 {
            limiter.record_failure("a", now);
        }
        let later = now + Duration::from_secs(61);
        assert!(limiter.retry_after("a", later).is_none());
    }

    #[test]
    fn retry_after_counts_down_from_the_oldest_failure() {
        let (limiter, now) = (limiter(), Instant::now());
        for _ in 0..3 {
            limiter.record_failure("a", now);
        }
        let later = now + Duration::from_secs(45);
        assert_eq!(
            limiter.retry_after("a", later),
            Some(Duration::from_secs(15))
        );
    }

    #[test]
    fn clear_forgets_a_key() {
        let (limiter, now) = (limiter(), Instant::now());
        for _ in 0..3 {
            limiter.record_failure("a", now);
        }
        limiter.clear("a");
        assert!(limiter.retry_after("a", now).is_none());
    }

    #[test]
    fn the_key_table_is_bounded() {
        let (limiter, now) = (limiter(), Instant::now());
        for n in 0..(MAX_KEYS + 50) {
            limiter.record_failure(&format!("k{n}"), now);
        }
        let size = limiter.failures.lock().unwrap().len();
        assert_eq!(size, MAX_KEYS);
    }
}
