//! Turns the pad's status samples into discrete session records.
//!
//! The pad zeroes its counters (time, distance, steps) whenever the belt stops, so each belt
//! run is seen as a fresh climb from zero. Runs separated by less than the coalesce window are
//! folded into one record: stepping off the belt for a moment, or letting it auto-stop, should
//! not split a walk into fragments that each fall below the Strava upload threshold.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};

use crate::model::{SessionRecord, WalkingSession, calories_for};

pub const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct SessionTracker {
    coalesce_window: SignedDuration,
    started_at: Option<Timestamp>,
    stopped_at: Option<Timestamp>,
    max_speed: f64,
    base_distance_km: f64,
    base_steps: u32,
    base_seconds: u32,
    /// Highest-elapsed sample of the belt run in progress. Counters grow monotonically while
    /// the belt runs, so this sample holds that run's final totals once it ends.
    peak: Option<WalkingSession>,
}

impl Default for SessionTracker {
    fn default() -> Self {
        Self::new(DEFAULT_COALESCE_WINDOW)
    }
}

impl SessionTracker {
    pub fn new(coalesce_window: Duration) -> Self {
        Self {
            coalesce_window: SignedDuration::try_from(coalesce_window)
                .expect("coalesce window fits"),
            started_at: None,
            stopped_at: None,
            max_speed: 0.0,
            base_distance_km: 0.0,
            base_steps: 0,
            base_seconds: 0,
            peak: None,
        }
    }

    /// Feeds a status sample, returning a record if it closed a session.
    pub fn on_sample(&mut self, sample: &WalkingSession, now: Timestamp) -> Option<SessionRecord> {
        if sample.is_active() {
            self.started_at.get_or_insert(now);
            // Restarted inside the coalesce window: bank the run that just ended, since the
            // counters we are about to see start over from zero.
            if self.stopped_at.take().is_some() {
                self.bank_run();
            }
            self.max_speed = self.max_speed.max(sample.speed);
            let peak_seconds = self.peak.as_ref().map_or(0, |p| p.elapsed_seconds);
            if sample.elapsed_seconds >= peak_seconds {
                self.peak = Some(sample.clone());
            }
        } else if self.started_at.is_some() && self.stopped_at.is_none() {
            self.stopped_at = Some(now);
        }
        self.tick(now)
    }

    /// Closes the session once the belt has been stopped for longer than the coalesce window.
    /// Needed as a separate entry point because the pad goes quiet while it sits idle,
    /// reporting the same values over and over.
    pub fn tick(&mut self, now: Timestamp) -> Option<SessionRecord> {
        let stopped = self.stopped_at?;
        if now.duration_since(stopped) < self.coalesce_window {
            return None;
        }
        self.close(stopped)
    }

    /// Closes whatever is in flight, for use when the app exits.
    pub fn finish(&mut self, now: Timestamp) -> Option<SessionRecord> {
        self.close(self.stopped_at.unwrap_or(now))
    }

    fn close(&mut self, ended_at: Timestamp) -> Option<SessionRecord> {
        self.bank_run();
        let record = match self.started_at {
            Some(started_at) if self.base_seconds > 0 && self.base_distance_km > 0.0 => {
                Some(SessionRecord {
                    started_at,
                    ended_at,
                    duration_seconds: self.base_seconds,
                    distance_km: self.base_distance_km,
                    steps: self.base_steps,
                    calories: calories_for(self.base_seconds),
                    avg_speed_kmh: self.base_distance_km / (f64::from(self.base_seconds) / 3600.0),
                    max_speed_kmh: self.max_speed,
                })
            }
            _ => None,
        };
        *self = Self {
            coalesce_window: self.coalesce_window,
            ..Self::new(Duration::ZERO)
        };
        record
    }

    fn bank_run(&mut self) {
        if let Some(p) = self.peak.take() {
            self.base_distance_km += p.distance_km;
            self.base_steps += p.steps;
            self.base_seconds += p.elapsed_seconds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BeltState;

    fn epoch() -> Timestamp {
        Timestamp::from_second(1_700_000_000).unwrap()
    }

    fn at(seconds: i64) -> Timestamp {
        epoch() + SignedDuration::from_secs(seconds)
    }

    fn running(seconds: u32, km: f64, steps: u32, speed: f64) -> WalkingSession {
        WalkingSession {
            state: BeltState::Running,
            speed,
            distance_km: km,
            steps,
            elapsed_seconds: seconds,
            ..Default::default()
        }
    }

    fn idle() -> WalkingSession {
        WalkingSession::default()
    }

    fn tracker() -> SessionTracker {
        SessionTracker::new(Duration::from_secs(60))
    }

    /// Walks the belt for `seconds` seconds starting at `from`, one sample per second.
    fn walk(t: &mut SessionTracker, from: i64, seconds: u32, speed: f64) -> Option<SessionRecord> {
        let mut closed = None;
        for i in 1..=seconds {
            let sample = running(i, f64::from(i) * 0.001, i, speed);
            if let Some(r) = t.on_sample(&sample, at(from + i64::from(i))) {
                closed = Some(r);
            }
        }
        closed
    }

    fn idle_for(t: &mut SessionTracker, from: i64, seconds: i64) -> Option<SessionRecord> {
        let mut closed = None;
        for i in 1..=seconds {
            if let Some(r) = t.on_sample(&idle(), at(from + i)) {
                closed = Some(r);
            }
        }
        closed
    }

    fn assert_km(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn closes_a_session_once_the_belt_has_been_stopped_past_the_window() {
        let mut t = tracker();
        assert!(walk(&mut t, 0, 100, 3.0).is_none());
        assert!(idle_for(&mut t, 100, 59).is_none());

        let record = idle_for(&mut t, 159, 5).unwrap();
        assert_eq!(record.duration_seconds, 100);
        assert_km(record.distance_km, 0.1);
        assert_eq!(record.steps, 100);
        assert_eq!(record.started_at, at(1));
        assert_eq!(record.ended_at, at(101));
    }

    #[test]
    fn folds_a_restart_inside_the_window_into_one_session() {
        let mut t = tracker();
        walk(&mut t, 0, 100, 3.0);
        idle_for(&mut t, 100, 30);
        // The second run is shorter than the first: keeping only the peak would drop it.
        assert!(walk(&mut t, 130, 40, 3.0).is_none());

        let record = t.finish(at(200)).unwrap();
        assert_eq!(record.duration_seconds, 140);
        assert_eq!(record.steps, 140);
        assert_km(record.distance_km, 0.14);
    }

    #[test]
    fn splits_a_restart_outside_the_window_into_separate_sessions() {
        let mut t = tracker();
        walk(&mut t, 0, 100, 3.0);
        let first = idle_for(&mut t, 100, 70).unwrap();
        assert_eq!(first.duration_seconds, 100);

        walk(&mut t, 200, 40, 3.0);
        let second = t.finish(at(250)).unwrap();
        assert_eq!(second.duration_seconds, 40);
        assert_eq!(second.steps, 40);
    }

    #[test]
    fn tracks_max_speed_across_folded_runs_and_resets_it_after() {
        let mut t = tracker();
        walk(&mut t, 0, 10, 5.5);
        idle_for(&mut t, 10, 10);
        walk(&mut t, 20, 10, 2.0);
        assert_eq!(t.finish(at(40)).unwrap().max_speed_kmh, 5.5);

        walk(&mut t, 100, 10, 2.0);
        assert_eq!(t.finish(at(120)).unwrap().max_speed_kmh, 2.0);
    }

    #[test]
    fn ignores_a_belt_that_never_moved() {
        let mut t = tracker();
        idle_for(&mut t, 0, 120);
        assert!(t.finish(at(130)).is_none());
    }

    #[test]
    fn tick_closes_a_session_while_samples_are_quiet() {
        let mut t = tracker();
        walk(&mut t, 0, 100, 3.0);
        idle_for(&mut t, 100, 1);
        assert!(t.tick(at(150)).is_none());
        assert_eq!(t.tick(at(170)).unwrap().duration_seconds, 100);
        assert!(t.tick(at(300)).is_none());
    }

    #[test]
    fn computes_average_speed() {
        let mut t = tracker();
        walk(&mut t, 0, 100, 3.0);
        let record = t.finish(at(150)).unwrap();
        // 0.1 km in 100 s.
        assert_km(record.avg_speed_kmh, 3.6);
    }
}
