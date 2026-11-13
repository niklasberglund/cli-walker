//! Remembers which sessions already reached Strava, keyed by session start time.
//!
//! Kept beside the append-only session log rather than inside it, so uploading never has to
//! rewrite `sessions.jsonl`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use jiff::Timestamp;

use crate::model::{SessionRecord, instant};
use crate::store::{DataDir, write_atomic};

/// Marks a session that reached Strava but whose activity id was never learned. Logging it
/// still matters: it is what stops the session being uploaded a second time.
const UNKNOWN_ACTIVITY_ID: i64 = 0;

#[derive(Debug)]
pub struct UploadLog {
    dir: DataDir,
    path: PathBuf,
    /// Keys as written, so entries this version cannot parse survive a rewrite.
    entries: BTreeMap<String, i64>,
    /// The same entries by instant. The Kotlin version wrote keys with `Instant.toString()`;
    /// comparing parsed instants means a formatting difference can never cause a re-upload.
    by_instant: BTreeMap<Timestamp, i64>,
}

impl UploadLog {
    pub fn load(dir: &DataDir) -> Self {
        let mut log = Self {
            dir: dir.clone(),
            path: dir.file("strava-uploads.json"),
            entries: BTreeMap::new(),
            by_instant: BTreeMap::new(),
        };
        log.reload();
        log
    }

    /// Picks up what another process (a `strava sync` while the dashboard is open, say) has
    /// recorded since this log was loaded.
    pub fn reload(&mut self) {
        let on_disk: BTreeMap<String, i64> = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        for (key, id) in on_disk {
            if let Ok(instant) = key.parse() {
                self.by_instant.insert(instant, id);
            }
            self.entries.insert(key, id);
        }
    }

    /// Deliberately keyed on presence, not on the id: a session logged with an unknown id has
    /// still been uploaded and must never be sent again.
    pub fn is_uploaded(&self, record: &SessionRecord) -> bool {
        self.by_instant.contains_key(&record.started_at)
    }

    /// The Strava activity id, if one is known.
    #[cfg(test)]
    pub fn activity_id_for(&self, record: &SessionRecord) -> Option<i64> {
        self.by_instant
            .get(&record.started_at)
            .copied()
            .filter(|&id| id != UNKNOWN_ACTIVITY_ID)
    }

    pub fn record(&mut self, record: &SessionRecord, activity_id: Option<i64>) -> Result<()> {
        // Rewriting from what this process loaded would drop entries written since.
        self.reload();
        let id = activity_id.unwrap_or(UNKNOWN_ACTIVITY_ID);
        self.entries.insert(instant::format(record.started_at), id);
        self.by_instant.insert(record.started_at, id);
        self.dir.ensure()?;
        write_atomic(
            &self.path,
            serde_json::to_string_pretty(&self.entries)?.as_bytes(),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(started_at: &str) -> SessionRecord {
        SessionRecord {
            started_at: started_at.parse().unwrap(),
            ended_at: started_at.parse().unwrap(),
            duration_seconds: 60,
            distance_km: 0.1,
            steps: 100,
            calories: 4,
            avg_speed_kmh: 6.0,
            max_speed_kmh: 6.0,
        }
    }

    #[test]
    fn recognises_sessions_uploaded_by_the_kotlin_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        fs::write(
            dir.file("strava-uploads.json"),
            r#"{
    "2026-10-21T19:43:05.512Z": 16234567890,
    "2026-10-22T07:00:00Z": 0
}"#,
        )
        .unwrap();
        let log = UploadLog::load(&dir);

        let known = record("2026-10-21T19:43:05.512Z");
        assert!(log.is_uploaded(&known));
        assert_eq!(log.activity_id_for(&known), Some(16_234_567_890));

        // Uploaded, but Strava never told us the id.
        let unknown_id = record("2026-10-22T07:00:00Z");
        assert!(log.is_uploaded(&unknown_id));
        assert_eq!(log.activity_id_for(&unknown_id), None);

        assert!(!log.is_uploaded(&record("2026-10-23T07:00:00Z")));
    }

    #[test]
    fn keeps_uploads_another_process_recorded_meanwhile() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let mut dashboard = UploadLog::load(&dir);
        let mut sync = UploadLog::load(&dir);

        let synced = record("2026-11-01T10:00:00Z");
        sync.record(&synced, Some(1)).unwrap();
        dashboard.reload();
        assert!(dashboard.is_uploaded(&synced));

        let walked = record("2026-11-02T10:00:00Z");
        UploadLog::load(&dir)
            .record(&record("2026-11-03T10:00:00Z"), Some(3))
            .unwrap();
        dashboard.record(&walked, Some(2)).unwrap();
        let on_disk = UploadLog::load(&dir);
        assert!(on_disk.is_uploaded(&synced));
        assert!(on_disk.is_uploaded(&walked));
        assert!(on_disk.is_uploaded(&record("2026-11-03T10:00:00Z")));
    }

    #[test]
    fn persists_new_uploads_and_keeps_unreadable_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        fs::write(dir.file("strava-uploads.json"), r#"{"garbage": 7}"#).unwrap();

        let mut log = UploadLog::load(&dir);
        let session = record("2026-11-12T18:00:00.250Z");
        log.record(&session, Some(42)).unwrap();

        let reloaded = UploadLog::load(&dir);
        assert_eq!(reloaded.activity_id_for(&session), Some(42));
        let text = fs::read_to_string(dir.file("strava-uploads.json")).unwrap();
        assert!(text.contains("\"garbage\": 7"), "{text}");
        assert!(text.contains("\"2026-11-12T18:00:00.250Z\": 42"), "{text}");
    }
}
