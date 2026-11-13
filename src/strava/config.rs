//! Strava credentials, OAuth tokens and upload preferences, in `strava.json` (mode 0600).
//!
//! The client secret is not really a secret in a distributed CLI: Strava's token endpoint
//! requires it, and there is no PKCE-style flow that avoids it. Each user registers their own
//! API application at <https://www.strava.com/settings/api>.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::{DataDir, write_private};

/// Field names match the Kotlin version, so an existing login keeps working.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StravaConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix epoch seconds at which `access_token` expires.
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub athlete_name: Option<String>,
    /// Mute uploads so they never reach followers' feeds. This is Strava's `hide_from_home`
    /// flag, not privacy: the activity is still on the athlete's profile.
    pub hide_from_feed: bool,
    /// Sessions shorter than this many km are not uploaded. 0 uploads everything.
    pub min_upload_distance_km: f64,
}

impl StravaConfig {
    pub fn is_authorized(&self) -> bool {
        self.refresh_token.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct StravaConfigStore {
    dir: DataDir,
    path: PathBuf,
}

impl StravaConfigStore {
    pub fn new(dir: &DataDir) -> Self {
        Self {
            dir: dir.clone(),
            path: dir.file("strava.json"),
        }
    }

    /// The stored config, or `None` if there is none or it cannot be read.
    pub fn load(&self) -> Option<StravaConfig> {
        let text = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, config: &StravaConfig) -> Result<()> {
        self.dir.ensure()?;
        let json = serde_json::to_string_pretty(config)?;
        write_private(&self.path, json.as_bytes())
            .with_context(|| format!("cannot write {}", self.path.display()))
    }

    pub fn clear(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_config_written_by_the_kotlin_version() {
        // kotlinx.serialization leaves out fields that hold their default value.
        let json = r#"{
    "clientId": "12345",
    "clientSecret": "s3cret",
    "accessToken": "acc",
    "refreshToken": "ref",
    "expiresAt": 1762900000,
    "athleteName": "Ada Walker",
    "minUploadDistanceKm": 0.5
}"#;
        let config: StravaConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.client_id, "12345");
        assert_eq!(config.refresh_token.as_deref(), Some("ref"));
        assert!(!config.hide_from_feed);
        assert_eq!(config.min_upload_distance_km, 0.5);
        assert!(config.is_authorized());
    }

    #[test]
    fn round_trips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let store = StravaConfigStore::new(&DataDir::new(tmp.path()));
        assert_eq!(store.load(), None);
        let config = StravaConfig {
            client_id: "1".into(),
            client_secret: "2".into(),
            hide_from_feed: true,
            ..Default::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load(), Some(config));
        assert!(!store.load().unwrap().is_authorized());
        store.clear();
        assert_eq!(store.load(), None);
    }
}
