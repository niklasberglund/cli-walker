//! A simulated WalkingPad for screenshots, recordings and end-to-end tests, plus the sample
//! data directory demo mode runs against.

use std::time::Duration;

use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use tokio::time::{MissedTickBehavior, interval, sleep};

use super::{ConnectionState, MAX_SPEED_KMH, MIN_SPEED_KMH, PadChannels, PadCommand, PadHandle};
use crate::model::{BeltState, SessionRecord, WalkingSession, calories_for};
use crate::store::DataDir;
use crate::strava::config::{StravaConfig, StravaConfigStore};

/// Average stride in metres, used to turn simulated distance into steps.
const STRIDE_M: f64 = 0.65;

/// Starts the simulated pad. Like the real one, it connects and then starts the belt.
pub fn spawn(start_speed: f64) -> PadHandle {
    let (handle, channels) = super::channels();
    let task = tokio::spawn(DemoPad::new().run(channels, start_speed));
    handle.attach(task)
}

/// Joins a walk already in progress, so the dashboard shows realistic totals from the
/// first frame.
struct DemoPad {
    speed: f64,
    distance_km: f64,
    elapsed_seconds: u32,
    stopped: bool,
}

impl DemoPad {
    fn new() -> Self {
        Self {
            speed: 0.0,
            distance_km: 1.38,
            elapsed_seconds: 27 * 60 + 41,
            stopped: false,
        }
    }

    async fn run(mut self, mut pad: PadChannels, start_speed: f64) {
        pad.connection.send_replace(ConnectionState::Scanning);
        sleep(Duration::from_millis(800)).await;
        pad.connection.send_replace(ConnectionState::Connecting);
        sleep(Duration::from_millis(500)).await;
        pad.connection.send_replace(ConnectionState::Connected);
        self.start(start_speed);
        self.publish(&pad);

        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if self.speed > 0.0 {
                        self.elapsed_seconds += 1;
                        self.distance_km += self.speed / 3600.0;
                    }
                }
                command = pad.commands.recv() => match command {
                    Some(PadCommand::StartBelt(speed)) => self.start(speed),
                    Some(PadCommand::StopBelt) => {
                        self.speed = 0.0;
                        self.stopped = true;
                    }
                    Some(PadCommand::SetSpeed(speed)) => {
                        if self.speed > 0.0 {
                            self.speed = speed.clamp(MIN_SPEED_KMH, MAX_SPEED_KMH);
                        }
                    }
                    Some(PadCommand::Shutdown(done)) => {
                        pad.connection.send_replace(ConnectionState::Disconnected);
                        let _ = done.send(());
                        return;
                    }
                    None => return,
                },
            }
            self.publish(&pad);
        }
    }

    fn start(&mut self, speed: f64) {
        // Like the real pad, a new belt run counts up from zero.
        if self.stopped {
            self.distance_km = 0.0;
            self.elapsed_seconds = 0;
            self.stopped = false;
        }
        self.speed = speed;
    }

    fn publish(&self, pad: &PadChannels) {
        let running = self.speed > 0.0;
        pad.session.send_replace(WalkingSession {
            state: if running {
                BeltState::Running
            } else {
                BeltState::Idle
            },
            raw_state: u8::from(running),
            speed: self.speed,
            distance_km: self.distance_km,
            steps: (self.distance_km * 1000.0 / STRIDE_M) as u32,
            elapsed_seconds: self.elapsed_seconds,
            ..Default::default()
        });
    }
}

/// Creates a throwaway data directory seeded with sample history and a fake Strava login,
/// so demo mode exercises the real storage code without touching the user's data.
pub fn prepare_data_dir() -> Result<DataDir> {
    let path = tempfile::Builder::new()
        .prefix("cli-walker-demo.")
        .tempdir()
        .context("cannot create the demo directory")?
        .keep();
    let dir = DataDir::new(path);
    let sessions = dir.sessions();
    for record in sample_history(Timestamp::now()) {
        sessions.append(&record)?;
    }
    StravaConfigStore::new(&dir).save(&StravaConfig {
        client_id: "demo".into(),
        client_secret: "demo".into(),
        access_token: Some("demo".into()),
        refresh_token: Some("demo".into()),
        athlete_name: Some("Demo Walker".into()),
        min_upload_distance_km: 1.0,
        ..Default::default()
    })?;
    Ok(dir)
}

/// About six weeks of plausible walks, one most days. The last is from earlier today, so
/// "This Month" is never empty.
pub fn sample_history(now: Timestamp) -> Vec<SessionRecord> {
    let mut random = SplitMix64(42);
    (0..=42)
        .rev()
        .filter_map(|days_ago: i64| {
            if days_ago > 0 && random.below(10) < 3 {
                return None;
            }
            let offset = SignedDuration::from_hours(days_ago * 24 + 3)
                + SignedDuration::from_mins(random.below(120) as i64);
            let start = now - offset;
            let avg_speed = 3.0 + random.below(16) as f64 / 10.0;
            let seconds = (20 + random.below(55)) as u32 * 60 + random.below(60) as u32;
            let distance = avg_speed * f64::from(seconds) / 3600.0;
            Some(SessionRecord {
                started_at: start,
                ended_at: start + SignedDuration::from_secs(i64::from(seconds)),
                duration_seconds: seconds,
                distance_km: distance,
                steps: (distance * 1000.0 / STRIDE_M) as u32,
                calories: calories_for(seconds),
                avg_speed_kmh: avg_speed,
                max_speed_kmh: avg_speed + (2 + random.below(6)) as f64 / 10.0,
            })
        })
        .collect()
}

/// A tiny deterministic generator, so the sample history is the same on every run.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_history_is_deterministic_and_ends_today() {
        let now = Timestamp::from_second(1_760_000_000).unwrap();
        let history = sample_history(now);
        assert_eq!(history, sample_history(now));
        assert!(history.len() > 20, "{} sessions", history.len());
        let last = history.last().unwrap();
        assert!(now.duration_since(last.started_at) < SignedDuration::from_hours(6));
        assert!(
            history
                .windows(2)
                .all(|w| w[0].started_at < w[1].started_at)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn simulated_walk_counts_up_and_restarts_from_zero() {
        let mut pad = spawn(3.6);
        pad.connection
            .wait_for(|s| *s == ConnectionState::Connected)
            .await
            .unwrap();
        let first = pad.session.borrow().clone();
        assert_eq!(first.speed, 3.6);
        assert!(first.is_active());

        tokio::time::sleep(Duration::from_secs(10)).await;
        let target = first.elapsed_seconds + 10;
        let later = pad
            .session
            .wait_for(|s| s.elapsed_seconds >= target)
            .await
            .unwrap()
            .clone();
        assert_eq!(later.elapsed_seconds, target);
        assert!((later.distance_km - (first.distance_km + 0.01)).abs() < 1e-9);

        pad.stop_belt();
        pad.session.wait_for(|s| !s.is_active()).await.unwrap();
        pad.start_belt(2.0);
        let restarted = pad
            .session
            .wait_for(|s| s.is_active())
            .await
            .unwrap()
            .clone();
        assert_eq!(restarted.elapsed_seconds, 0);

        pad.shutdown(Duration::from_secs(1)).await;
    }
}
