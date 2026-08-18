//! Simple in-memory sliding-window rate limiter.
//!
//! One instance per bucket (bot commands, HTTP API). Memory cost is bounded:
//! hits are only recorded for allowed requests and pruned once they fall
//! outside the longest window.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Multi-window sliding-window limiter: 'windows' is a list of
/// (max_requests, window). A key is blocked when ANY window is exceeded.
pub struct RateLimiter {
    windows: Vec<(u64, Duration)>,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    /// 'windows' must be non-empty; typical: [(15, 10s), (60, 60s)].
    pub fn new(windows: &[(u64, Duration)]) -> Self {
        Self {
            windows: windows
                .iter()
                .filter(|(limit, _)| *limit > 0)
                .copied()
                .collect(),
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Check-and-record: Ok(()) when the request is allowed (and recorded),
    /// Err(retry_after_secs) when any window is exceeded.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        if self.windows.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        let mut map = self.hits.lock().unwrap();
        let entry = map.entry(key.to_string()).or_default();
        for (limit, window) in &self.windows {
            let count = entry
                .iter()
                .filter(|t| now.duration_since(**t) < *window)
                .count() as u64;
            if count >= *limit {
                return Err(window.as_secs().max(1));
            }
        }
        entry.push(now);
        let longest = self
            .windows
            .iter()
            .map(|(_, w)| *w)
            .max()
            .unwrap_or(Duration::ZERO);
        entry.retain(|t| now.duration_since(*t) < longest);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_limit_per_window() {
        let rl = RateLimiter::new(&[(3, Duration::from_secs(60))]);
        assert!(rl.check("a").is_ok());
        assert!(rl.check("a").is_ok());
        assert!(rl.check("a").is_ok());
        assert_eq!(rl.check("a"), Err(60));
        // Other keys are unaffected.
        assert!(rl.check("b").is_ok());
    }

    #[test]
    fn shortest_window_wins() {
        let rl = RateLimiter::new(&[(1, Duration::from_secs(10)), (5, Duration::from_secs(60))]);
        assert!(rl.check("a").is_ok());
        assert_eq!(rl.check("a"), Err(10));
    }
}
