/// Replay protection via sliding window bitmap.
///
/// Each session maintains a sliding window of recently seen frame counters.
/// When a frame arrives, its counter is checked:
/// - If counter > window_max: advance window, mark as seen
/// - If counter within window and not seen: mark as seen, accept
/// - If counter within window and already seen: reject (replay)
/// - If counter < window_base: reject (too old)
///
/// Window size is configurable (default 64 frames).

use std::collections::HashSet;

pub struct ReplayWindow {
    window_size: u64,
    window_base: u64,
    seen: HashSet<u64>,
    max_counter: Option<u64>,
}

impl ReplayWindow {
    pub fn new(window_size: u64) -> Self {
        Self {
            window_size,
            window_base: 0,
            seen: HashSet::with_capacity(window_size as usize),
            max_counter: None,
        }
    }

    /// Checks if a counter is new (not replayed).
    /// Returns true if accepted, false if replayed.
    pub fn check(&mut self, counter: u64) -> bool {
        match self.max_counter {
            None => {
                self.max_counter = Some(counter);
                self.window_base = counter.saturating_sub(self.window_size);
                self.seen.insert(counter);
                true
            }
            Some(max) => {
                if counter > max {
                    let new_base = counter.saturating_sub(self.window_size);
                    if new_base > self.window_base {
                        self.seen.retain(|&c| c >= new_base);
                        self.window_base = new_base;
                    }
                    self.max_counter = Some(counter);
                    self.seen.insert(counter);
                    true
                } else if counter >= self.window_base {
                    if self.seen.contains(&counter) {
                        false
                    } else {
                        self.seen.insert(counter);
                        true
                    }
                } else {
                    false
                }
            }
        }
    }

    pub fn current_max(&self) -> u64 {
        self.max_counter.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accept_new_counters() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(0));
        assert!(w.check(1));
        assert!(w.check(2));
        assert!(w.check(100));
    }

    #[test]
    fn test_reject_replay() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(5));
        assert!(!w.check(5)); // replay
    }

    #[test]
    fn test_reject_old_counter() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(100));
        // 50 is within window (100-64=36), not seen before — accepted
        assert!(w.check(50));
        // 30 is below window base (36) — rejected as too old
        assert!(!w.check(30));
    }

    #[test]
    fn test_window_advance_clears_old() {
        let mut w = ReplayWindow::new(64);
        for i in 0..200 {
            assert!(w.check(i));
        }
        // Counter 0 should now be too old (200 - 64 = 136)
        assert!(!w.check(0));
    }

    #[test]
    fn test_out_of_order_within_window() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(100));
        assert!(w.check(95)); // within window, not seen
        assert!(!w.check(95)); // replay
        assert!(w.check(99)); // within window, not seen
    }

    #[test]
    fn test_large_jump_clears_window() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(0));
        assert!(w.check(1));
        assert!(w.check(2));
        // Jump far ahead
        assert!(w.check(1000));
        // Old counters should be rejected
        assert!(!w.check(1));
    }

    #[test]
    fn test_zero_counter() {
        let mut w = ReplayWindow::new(64);
        assert!(w.check(0));
        assert!(!w.check(0));
    }
}
