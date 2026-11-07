//! A one-shot countdown started from the menu.
//!
//! The end time is wall-clock time, so a timer keeps counting while the Mac
//! sleeps and finishes as soon as it wakes. It is not saved: quitting the
//! app drops it.

use std::time::{Duration, SystemTime};

/// Durations offered in the Timer menu, in minutes.
pub const PRESET_MINUTES: [u64; 4] = [5, 15, 25, 60];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    ends_at: SystemTime,
}

impl Timer {
    pub fn start(minutes: u64, now: SystemTime) -> Timer {
        Timer {
            ends_at: now + Duration::from_secs(minutes * 60),
        }
    }

    /// Time left, or `None` once the timer has finished.
    pub fn remaining(&self, now: SystemTime) -> Option<Duration> {
        let left = self.ends_at.duration_since(now).ok()?;
        (!left.is_zero()).then_some(left)
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up. Partial seconds round up, so the
/// display never reads `0:00` while time is left.
pub fn format_remaining(left: Duration) -> String {
    let total = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_reads_like_a_clock_and_never_shows_zero_early() {
        assert_eq!(format_remaining(Duration::from_secs(25 * 60)), "25:00");
        assert_eq!(format_remaining(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_remaining(Duration::from_millis(300)), "0:01");
    }

    #[test]
    fn a_timer_finishes_at_its_end_time() {
        let start = SystemTime::UNIX_EPOCH;
        let timer = Timer::start(5, start);

        assert!(timer.remaining(start + Duration::from_secs(299)).is_some());
        assert_eq!(timer.remaining(start + Duration::from_secs(300)), None);
    }
}
