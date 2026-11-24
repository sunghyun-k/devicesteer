//! Plays a live touch's pointer path back at a steady rate (PITFALLS #18).
//!
//! A client's moves reach the daemon unevenly: the viewer sends one per pointer event, and the
//! page, the IPC and the socket each add a few milliseconds of jitter. Written to the device as
//! they come, a 60 Hz path against the device's 60 Hz display sometimes puts two moves in one
//! display frame and none in the next, and the list judders. So the daemon keeps the path with
//! each point's time and reports where the finger was `delay` ago, interpolated, on a fixed tick.
//!
//! The delay has to cover the gap to the next point, or the playback catches up with the path,
//! stops, and jumps when the point comes. It grows to the client's recent spacing (a 30 Hz client
//! needs more than a 100 Hz mouse), slowly, so the playback only slows down a little and never
//! runs backwards. It never shrinks within a touch: following the jitter both ways kept changing
//! the playback speed, and the list's speed with it, which measured worse than a fixed delay.

use std::collections::VecDeque;

/// One point of the path: when (ms, on the daemon's clock) and where (screen points).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    at: f64,
    point: (f64, f64),
}

#[derive(Debug, Clone)]
pub struct Resampler {
    samples: VecDeque<Sample>,
    /// Daemon time minus client time, the smallest seen in this touch: the client's timestamps
    /// space the points as the pointer moved, not as they happened to arrive.
    offset: Option<f64>,
    /// The current delay (ms), between `min_delay` and [`MAX_DELAY`].
    delay: f64,
    min_delay: f64,
    /// Recent spacing of the client's points (ms), pauses left out.
    gaps: VecDeque<f64>,
    /// When `sample` was last called, to pace changes of the delay.
    sampled: Option<f64>,
}

/// Points kept; older ones are behind the playback point anyway.
const KEEP: usize = 64;
/// Gaps the delay follows.
const GAPS: usize = 8;
/// A longer gap is the pointer resting, not its pace.
const PAUSE: f64 = 80.0;
/// Upper bound of the delay (ms): beyond it the finger lags too far behind the pointer.
pub const MAX_DELAY: f64 = 50.0;
/// Margin over the largest recent gap (ms), for the jitter of arrival.
const MARGIN: f64 = 4.0;
/// How fast the delay grows, in ms per ms: the playback runs at 0.9 times real speed while it does.
const SLEW: f64 = 0.1;

impl Resampler {
    /// A path that starts at `point`, at daemon time `now` (ms).
    /// `min_delay` (ms) is also where the delay starts.
    pub fn new(now: f64, client_t: Option<f64>, point: (f64, f64), min_delay: f64) -> Self {
        let mut r = Self {
            samples: VecDeque::new(),
            offset: None,
            delay: min_delay,
            min_delay,
            gaps: VecDeque::new(),
            sampled: None,
        };
        r.push(now, client_t, point);
        r
    }

    /// Adds the point a client reported at `client_t` (its own clock, ms) or, without one, at
    /// `now` when it arrived.
    pub fn push(&mut self, now: f64, client_t: Option<f64>, point: (f64, f64)) {
        let at = match client_t {
            Some(t) => {
                let off = self.offset.map_or(now - t, |o| o.min(now - t));
                self.offset = Some(off);
                t + off
            }
            None => now,
        };
        // A client clock that went backwards (or reordering) must not bend the path back.
        let at = self.samples.back().map_or(at, |s| at.max(s.at));
        if let Some(prev) = self.samples.back()
            && at - prev.at < PAUSE
        {
            self.gaps.push_back(at - prev.at);
            if self.gaps.len() > GAPS {
                self.gaps.pop_front();
            }
        }
        self.samples.push_back(Sample { at, point });
        while self.samples.len() > KEEP {
            self.samples.pop_front();
        }
    }

    /// Where the finger was `delay` before `now`: interpolated between the points around that
    /// time, the first point before the path starts, the last one once it is played out.
    pub fn sample(&mut self, now: f64) -> (f64, f64) {
        let target = (self.gaps.iter().copied().fold(0.0, f64::max) + MARGIN)
            .clamp(self.min_delay, MAX_DELAY);
        let step = self.sampled.map_or(0.0, |p| (now - p).max(0.0) * SLEW);
        self.delay += (target - self.delay).clamp(0.0, step);
        self.sampled = Some(now);
        let t = now - self.delay;
        // Drop points that the playback point has passed, keeping one before it.
        while self.samples.len() > 1 && self.samples[1].at <= t {
            self.samples.pop_front();
        }
        let a = self.samples[0];
        match self.samples.get(1) {
            Some(b) if t > a.at => {
                let u = ((t - a.at) / (b.at - a.at)).clamp(0.0, 1.0);
                (
                    a.point.0 + (b.point.0 - a.point.0) * u,
                    a.point.1 + (b.point.1 - a.point.1) * u,
                )
            }
            _ => a.point,
        }
    }

    /// Daemon time (ms) at which [`Self::sample`] reaches the last point, at the current delay.
    pub fn done_at(&self) -> f64 {
        self.samples.back().map_or(0.0, |s| s.at) + self.delay
    }

    /// The last point the client sent.
    pub fn last(&self) -> (f64, f64) {
        self.samples
            .back()
            .expect("a path has at least one point")
            .point
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_back_delayed_and_interpolated() {
        let mut r = Resampler::new(0.0, None, (0.0, 0.0), 20.0);
        r.push(16.0, None, (0.0, 16.0));
        r.push(32.0, None, (0.0, 32.0));
        assert_eq!(r.sample(10.0), (0.0, 0.0));
        assert_eq!(r.sample(28.0), (0.0, 8.0));
        assert_eq!(r.sample(44.0), (0.0, 24.0));
        assert_eq!(r.sample(60.0), (0.0, 32.0));
        assert_eq!(r.done_at(), 52.0);
    }

    #[test]
    fn client_times_space_the_points() {
        // Sent 16 ms apart, arrived 1 ms and 12 ms late: the path keeps the 16 ms spacing.
        let mut r = Resampler::new(101.0, Some(100.0), (0.0, 0.0), 20.0);
        r.push(128.0, Some(116.0), (0.0, 16.0));
        // Offset stays at the smallest delay seen (1 ms), so the second point is at 117.
        assert_eq!(r.sample(109.0 + 20.0), (0.0, 8.0));
        assert_eq!(r.sample(117.0 + 20.0), (0.0, 16.0));
    }

    #[test]
    fn delay_follows_a_slow_client_without_running_backwards() {
        // A 30 Hz client: points 33 ms apart, sampled on an 8 ms tick.
        let mut r = Resampler::new(0.0, None, (0.0, 0.0), 20.0);
        let mut last = 0.0;
        let mut stalls = 0;
        for tick in 1..120 {
            let now = f64::from(tick) * 8.0;
            let sent = (now / 33.0).floor() as i32;
            while r.samples.back().unwrap().at < f64::from(sent) * 33.0 {
                let at = r.samples.back().unwrap().at + 33.0;
                r.push(at, None, (0.0, at));
            }
            let y = r.sample(now).1;
            assert!(y >= last, "ran backwards at {now}: {y} < {last}");
            if tick > 40 && y == last {
                stalls += 1;
            }
            last = y;
        }
        assert!(r.delay > 33.0 && r.delay <= MAX_DELAY, "delay {}", r.delay);
        assert_eq!(stalls, 0, "playback caught up with the path");
    }

    #[test]
    fn time_never_runs_backwards() {
        let mut r = Resampler::new(0.0, None, (0.0, 0.0), 10.0);
        r.push(20.0, Some(5.0), (0.0, 10.0));
        r.push(21.0, Some(1.0), (0.0, 20.0));
        assert!(r.done_at() >= 30.0);
        assert_eq!(r.sample(100.0), (0.0, 20.0));
    }
}
