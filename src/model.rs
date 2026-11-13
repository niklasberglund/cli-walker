//! Session data: the pad's live status and the records saved once a walk ends.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// The A1 Pro stops the belt after two hours of continuous running and then locks itself in a
/// 30-minute REST countdown. None of this is visible over BLE, so the cap is tracked from
/// elapsed time.
pub const MAX_RUN_SECONDS: u32 = 2 * 60 * 60;
pub const AUTO_STOP_WARNING_SECONDS: u32 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BeltState {
    #[default]
    Idle,
    Running,
    Starting,
    Standby,
    Accelerating,
    /// Seen for about a second right after the start command, before the belt moves.
    Ramping,
    /// A state byte the protocol docs do not describe; `WalkingSession::raw_state` keeps it.
    Unknown,
}

impl BeltState {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Idle,
            1 => Self::Running,
            4 => Self::Starting,
            5 => Self::Standby,
            7 => Self::Ramping,
            9 => Self::Accelerating,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BeltMode {
    Automatic = 0,
    #[default]
    Manual = 1,
    Sleep = 2,
}

impl BeltMode {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Automatic,
            2 => Self::Sleep,
            _ => Self::Manual,
        }
    }
}

/// One status sample from the pad. The counters reset whenever the belt stops.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WalkingSession {
    pub state: BeltState,
    pub raw_state: u8,
    pub mode: BeltMode,
    /// km/h
    pub speed: f64,
    pub distance_km: f64,
    pub steps: u32,
    pub elapsed_seconds: u32,
}

impl WalkingSession {
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            BeltState::Running | BeltState::Starting | BeltState::Accelerating | BeltState::Ramping
        )
    }

    pub fn formatted_speed(&self) -> String {
        format!("{} km/h", truncate_1(self.speed))
    }

    pub fn formatted_distance(&self) -> String {
        format!("{} km", truncate_2(self.distance_km))
    }

    pub fn calories_estimate(&self) -> u32 {
        calories_for(self.elapsed_seconds)
    }

    /// Seconds until the pad's run cap stops the belt, once the warning window has begun.
    pub fn seconds_until_auto_stop(&self) -> Option<u32> {
        let remaining = i64::from(MAX_RUN_SECONDS) - i64::from(self.elapsed_seconds);
        (self.is_active() && remaining <= i64::from(AUTO_STOP_WARNING_SECONDS))
            .then(|| remaining.max(0) as u32)
    }
}

/// A finished walk, as stored one per line in `sessions.jsonl`. Field names and timestamp
/// format match what the Kotlin version wrote, so existing history keeps loading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(with = "instant")]
    pub started_at: Timestamp,
    #[serde(with = "instant")]
    pub ended_at: Timestamp,
    pub duration_seconds: u32,
    pub distance_km: f64,
    pub steps: u32,
    pub calories: u32,
    pub avg_speed_kmh: f64,
    pub max_speed_kmh: f64,
}

pub fn calories_for(seconds: u32) -> u32 {
    let hours = f64::from(seconds) / 3600.0;
    (3.5 * 70.0 * hours * 1.05 / 60.0 * 60.0) as u32
}

/// `m:ss`-style duration: `mm:ss` under an hour, `h:mm:ss` from then on.
pub fn format_duration(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// One decimal, truncated rather than rounded (3.79 shows as 3.7), as the Kotlin UI did.
pub fn truncate_1(value: f64) -> String {
    let tenths = (value * 10.0) as i64;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Two decimals, truncated rather than rounded.
pub fn truncate_2(value: f64) -> String {
    let hundredths = (value * 100.0) as i64;
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Timestamps in the format kotlinx-datetime's `Instant.toString()` produces: UTC, with the
/// fraction of a second in groups of three digits, or none. The Strava upload log is keyed
/// by this string, so it has to match what the Kotlin version wrote.
pub mod instant {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn format(ts: Timestamp) -> String {
        let whole = Timestamp::from_second(ts.as_second()).expect("whole-second timestamp");
        let base = whole.strftime("%Y-%m-%dT%H:%M:%S");
        let nanos = ts.subsec_nanosecond();
        let fraction = match nanos {
            0 => String::new(),
            n if n % 1_000_000 == 0 => format!(".{:03}", n / 1_000_000),
            n if n % 1_000 == 0 => format!(".{:06}", n / 1_000),
            n => format!(".{n:09}"),
        };
        format!("{base}{fraction}Z")
    }

    pub fn serialize<S: Serializer>(ts: &Timestamp, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(*ts))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(state: BeltState, seconds: u32) -> WalkingSession {
        WalkingSession {
            state,
            elapsed_seconds: seconds,
            ..Default::default()
        }
    }

    #[test]
    fn no_auto_stop_warning_before_window() {
        let s = session(
            BeltState::Running,
            MAX_RUN_SECONDS - AUTO_STOP_WARNING_SECONDS - 1,
        );
        assert_eq!(s.seconds_until_auto_stop(), None);
    }

    #[test]
    fn auto_stop_warning_counts_down_in_window() {
        assert_eq!(
            session(BeltState::Running, 6900).seconds_until_auto_stop(),
            Some(AUTO_STOP_WARNING_SECONDS)
        );
        assert_eq!(
            session(BeltState::Running, MAX_RUN_SECONDS - 1).seconds_until_auto_stop(),
            Some(1)
        );
        assert_eq!(
            session(BeltState::Running, MAX_RUN_SECONDS + 3).seconds_until_auto_stop(),
            Some(0)
        );
    }

    #[test]
    fn no_auto_stop_warning_when_belt_stopped() {
        assert_eq!(
            session(BeltState::Idle, 7000).seconds_until_auto_stop(),
            None
        );
    }

    #[test]
    fn formats_like_the_kotlin_ui() {
        assert_eq!(format_duration(59), "00:59");
        assert_eq!(format_duration(27 * 60 + 45), "27:45");
        assert_eq!(format_duration(3600 + 8 * 60 + 1), "1:08:01");
        assert_eq!(truncate_1(3.79), "3.7");
        assert_eq!(truncate_2(1.389), "1.38");
        assert_eq!(truncate_2(0.05), "0.05");
        assert_eq!(calories_for(27 * 60 + 45), 118);
    }

    #[test]
    fn instants_match_kotlinx_datetime() {
        let at = |s: &str| s.parse::<Timestamp>().unwrap();
        for text in [
            "2026-10-21T19:43:05Z",
            "2026-10-21T19:43:05.120Z",
            "2026-10-21T19:43:05.123456Z",
            "2026-10-21T19:43:05.123456789Z",
        ] {
            assert_eq!(instant::format(at(text)), text);
        }
    }

    #[test]
    fn reads_a_record_written_by_the_kotlin_version() {
        let line = r#"{"startedAt":"2026-10-21T19:43:05.512Z","endedAt":"2026-10-21T20:51:22.008Z","durationSeconds":4097,"distanceKm":4.77,"steps":7353,"calories":250,"avgSpeedKmh":4.2,"maxSpeedKmh":4.8}"#;
        let record: SessionRecord = serde_json::from_str(line).unwrap();
        assert_eq!(record.steps, 7353);
        assert_eq!(serde_json::to_string(&record).unwrap(), line);
    }
}
