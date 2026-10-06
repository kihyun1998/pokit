//! Aligning pokit's clock with the page's (`performance.timeOrigin + performance.now()`).

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many round trips one alignment takes.
pub const SAMPLES: usize = 20;

/// Milliseconds since the Unix epoch at `at`, on a monotonic base fixed at first use.
pub fn local_ms(at: Instant) -> f64 {
    static BASE: OnceLock<(Instant, f64)> = OnceLock::new();
    let (base, epoch) = *BASE.get_or_init(|| {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        (Instant::now(), epoch)
    });
    if at >= base {
        epoch + (at - base).as_secs_f64() * 1000.0
    } else {
        epoch - (base - at).as_secs_f64() * 1000.0
    }
}

/// One round trip: when pokit asked, what the page's clock said, and when the answer came back.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub sent: f64,
    pub page: f64,
    pub received: f64,
}

/// The page clock minus pokit's, and half the round trip it was taken from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub offset_ms: f64,
    pub uncertainty_ms: f64,
    pub samples: usize,
}

impl Clock {
    /// A time on pokit's clock, on the page's.
    pub fn page_time(&self, local: f64) -> f64 {
        local + self.offset_ms
    }

    pub fn to_json(self) -> Value {
        json!({
            "offset_ms": round3(self.offset_ms),
            "uncertainty_ms": round3(self.uncertainty_ms),
            "samples": self.samples,
        })
    }
}

/// Keeps the sample with the shortest round trip and takes the offset at its midpoint.
pub fn align(samples: &[Sample]) -> Option<Clock> {
    let best = samples
        .iter()
        .filter(|s| s.received >= s.sent)
        .min_by(|a, b| (a.received - a.sent).total_cmp(&(b.received - b.sent)))?;
    let round_trip = best.received - best.sent;
    Some(Clock {
        offset_ms: best.page - (best.sent + round_trip / 2.0),
        uncertainty_ms: round_trip / 2.0,
        samples: samples.len(),
    })
}

/// `v` rounded to three decimals, for output.
pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shortest_round_trip_wins_and_the_offset_is_taken_at_its_midpoint() {
        let samples = [
            Sample {
                sent: 1000.0,
                page: 5020.0,
                received: 1040.0,
            },
            Sample {
                sent: 2000.0,
                page: 6003.0,
                received: 2004.0,
            },
            Sample {
                sent: 3000.0,
                page: 7050.0,
                received: 3090.0,
            },
        ];
        let c = align(&samples).unwrap();
        assert_eq!(c.offset_ms, 4001.0);
        assert_eq!(c.uncertainty_ms, 2.0);
        assert_eq!(c.samples, 3);
        assert_eq!(c.page_time(2500.0), 6501.0);
    }

    #[test]
    fn no_usable_sample_gives_no_alignment() {
        assert_eq!(align(&[]), None);
        let backwards = Sample {
            sent: 10.0,
            page: 0.0,
            received: 5.0,
        };
        assert_eq!(align(&[backwards]), None);
    }

    #[test]
    fn the_local_clock_is_monotonic_and_near_the_wall_clock() {
        let a = Instant::now();
        let b = a + std::time::Duration::from_millis(250);
        assert_eq!(round3(local_ms(b) - local_ms(a)), 250.0);
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;
        assert!((local_ms(Instant::now()) - wall).abs() < 1000.0);
    }
}
