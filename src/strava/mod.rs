//! Strava upload: OAuth login, TCX uploads, and the settings and log that go with them.

pub mod api;
pub mod config;
pub mod loopback;
pub mod tcx;
pub mod upload_log;

use std::time::Duration;

use jiff::tz::TimeZone;

use self::api::{Endpoints, SCOPE, StravaApi};
use self::config::{StravaConfig, StravaConfigStore};
use self::loopback::{LoopbackServer, RedirectResult};
use self::upload_log::UploadLog;
use crate::model::SessionRecord;
use crate::store::DataDir;

/// Default port for the loopback OAuth listener.
pub const DEFAULT_CALLBACK_PORT: u16 = 8721;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResult {
    Success { athlete_name: Option<String> },
    Failure(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UploadResult {
    /// `activity_id` is `None` when Strava accepted the file but had not finished processing
    /// it. `warning` flags anything that failed after it was accepted.
    Uploaded {
        activity_id: Option<i64>,
        warning: Option<String>,
    },
    AlreadyUploaded,
    NotConnected,
    /// The session is shorter than the configured minimum upload distance.
    TooShort {
        distance_km: f64,
        threshold_km: f64,
    },
    Failed(String),
}

pub struct LoginOptions {
    pub port: u16,
    pub timeout: Duration,
    pub open_browser: bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            port: DEFAULT_CALLBACK_PORT,
            timeout: Duration::from_secs(180),
            open_browser: true,
        }
    }
}

/// Ties together config storage, the OAuth loopback flow, and uploads.
pub struct StravaService {
    store: StravaConfigStore,
    upload_log: UploadLog,
    api: StravaApi,
    endpoints: Endpoints,
}

impl StravaService {
    pub fn new(dir: &DataDir) -> Self {
        Self::with_endpoints(dir, Endpoints::default())
    }

    pub fn with_endpoints(dir: &DataDir, endpoints: Endpoints) -> Self {
        let store = StravaConfigStore::new(dir);
        Self {
            api: StravaApi::new(store.clone(), endpoints.clone()),
            upload_log: UploadLog::load(dir),
            store,
            endpoints,
        }
    }

    #[cfg(test)]
    pub fn api_mut(&mut self) -> &mut StravaApi {
        &mut self.api
    }

    pub fn config(&self) -> Option<StravaConfig> {
        self.store.load()
    }

    pub fn is_connected(&self) -> bool {
        self.config().is_some_and(|c| c.is_authorized())
    }

    pub fn logout(&self) {
        self.store.clear();
    }

    /// Turns feed muting on or off for future uploads. Returns `false` if there is no config
    /// yet to attach the setting to.
    pub fn set_hide_from_feed(&self, hide: bool) -> anyhow::Result<bool> {
        let Some(config) = self.store.load() else {
            return Ok(false);
        };
        self.store.save(&StravaConfig {
            hide_from_feed: hide,
            ..config
        })?;
        Ok(true)
    }

    /// Minimum session distance (km) that is uploaded; 0 uploads everything.
    pub fn min_upload_distance_km(&self) -> f64 {
        self.store.load().map_or(0.0, |c| c.min_upload_distance_km)
    }

    /// Returns `false` when there is no config yet (before `strava login`).
    pub fn set_min_upload_distance_km(&self, km: f64) -> anyhow::Result<bool> {
        let Some(config) = self.store.load() else {
            return Ok(false);
        };
        self.store.save(&StravaConfig {
            min_upload_distance_km: km.max(0.0),
            ..config
        })?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn activity_id_for(&self, record: &SessionRecord) -> Option<i64> {
        self.upload_log.activity_id_for(record)
    }

    /// Runs the authorization code flow. `on_prompt` gets the authorize URL so the caller
    /// can show it before the browser opens.
    pub async fn login(
        &self,
        client_id: &str,
        client_secret: &str,
        options: LoginOptions,
        on_prompt: impl FnOnce(&str, u16),
    ) -> LoginResult {
        // Re-authorizing (for a new scope, say) must not reset preferences, and must not
        // drop working tokens before the new grant has actually succeeded.
        let existing = self.store.load();
        let config = StravaConfig {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            hide_from_feed: existing.as_ref().is_some_and(|c| c.hide_from_feed),
            min_upload_distance_km: existing.as_ref().map_or(0.0, |c| c.min_upload_distance_km),
            ..Default::default()
        };
        if existing.is_none()
            && let Err(e) = self.store.save(&config)
        {
            return LoginResult::Failure(format!("{e:#}"));
        }

        let server = match LoopbackServer::bind(options.port).await {
            Ok(server) => server,
            Err(reason) => return LoginResult::Failure(reason),
        };
        let port = server.port();
        let redirect_uri = format!("http://localhost:{port}/callback");
        let authorize_url = format!(
            "{}/authorize?client_id={}&redirect_uri={}&response_type=code&approval_prompt=auto&scope={}",
            self.endpoints.oauth,
            encode(client_id),
            encode(&redirect_uri),
            encode(SCOPE),
        );
        on_prompt(&authorize_url, port);
        if options.open_browser {
            let _ = webbrowser::open(&authorize_url);
        }

        match server.await_redirect(options.timeout).await {
            RedirectResult::Code { code, scopes } => {
                // SCOPE is a comma-joined request; Strava grants each scope individually.
                let missing: Vec<&str> = SCOPE
                    .split(',')
                    .filter(|s| !scopes.iter().any(|g| g == s))
                    .collect();
                if !missing.is_empty() {
                    return LoginResult::Failure(format!(
                        "Strava did not grant {} — re-run login and leave every permission checkbox ticked.",
                        missing.join(", ")
                    ));
                }
                match self.api.exchange_code(&config, &code).await {
                    Ok(updated) => LoginResult::Success {
                        athlete_name: updated.athlete_name,
                    },
                    Err(e) => LoginResult::Failure(e.to_string()),
                }
            }
            RedirectResult::Denied(error) => {
                LoginResult::Failure(format!("authorization was declined ({error})"))
            }
            RedirectResult::Failed(reason) => LoginResult::Failure(reason),
            RedirectResult::TimedOut => {
                LoginResult::Failure("timed out waiting for the browser redirect".into())
            }
        }
    }

    /// Uploads one session, skipping sessions already sent.
    pub async fn upload(&mut self, record: &SessionRecord, ignore_threshold: bool) -> UploadResult {
        if self.upload_log.is_uploaded(record) {
            return UploadResult::AlreadyUploaded;
        }
        let Some(config) = self.store.load().filter(StravaConfig::is_authorized) else {
            return UploadResult::NotConnected;
        };
        if !ignore_threshold && record.distance_km < config.min_upload_distance_km {
            return UploadResult::TooShort {
                distance_km: record.distance_km,
                threshold_km: config.min_upload_distance_km,
            };
        }
        let name = activity_name(record, &TimeZone::system());
        match self.api.upload_activity(&config, record, &name).await {
            Ok(upload) => {
                let mut warning = upload.warning;
                if let Err(e) = self.upload_log.record(record, upload.activity_id) {
                    // The activity exists; failing here would upload it again next time.
                    warning = Some(format!(
                        "uploaded, but could not record it locally, so a later sync may upload it again: {e:#}"
                    ));
                }
                UploadResult::Uploaded {
                    activity_id: upload.activity_id,
                    warning,
                }
            }
            Err(e) => UploadResult::Failed(e.to_string()),
        }
    }

    /// Sessions never uploaded, oldest first. Sessions below the minimum distance are left out
    /// unless `ignore_threshold` is set, so the pending count matches what a sync would send.
    pub fn pending(&self, records: &[SessionRecord], ignore_threshold: bool) -> Vec<SessionRecord> {
        let threshold = if ignore_threshold {
            0.0
        } else {
            self.min_upload_distance_km()
        };
        let mut pending: Vec<SessionRecord> = records
            .iter()
            .filter(|r| !self.upload_log.is_uploaded(r) && r.distance_km >= threshold)
            .cloned()
            .collect();
        pending.sort_by_key(|r| r.started_at);
        pending
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Names the activity the way Strava names its own: by time of day.
pub fn activity_name(record: &SessionRecord, tz: &TimeZone) -> String {
    let hour = record.started_at.to_zoned(tz.clone()).hour();
    let part_of_day = match hour {
        5..=11 => "Morning",
        12..=13 => "Lunch",
        14..=17 => "Afternoon",
        18..=21 => "Evening",
        _ => "Night",
    };
    format!("{part_of_day} Treadmill Walk")
}

/// A distance threshold in its most readable unit: metres below 1 km, km above. 0 is "off".
pub fn format_threshold(km: f64) -> String {
    if km <= 0.0 {
        return "off".into();
    }
    if km < 1.0 {
        return format!("{} m", (km * 1000.0) as i64);
    }
    let hundredths = (km * 100.0) as i64;
    let fraction = hundredths % 100;
    if fraction == 0 {
        format!("{} km", hundredths / 100)
    } else {
        let digits = format!("{fraction:02}");
        format!("{}.{} km", hundredths / 100, digits.trim_end_matches('0'))
    }
}

/// Parses a typed threshold into km: "500m", "1.2km", a bare number (km), or "off", "none"
/// and "0". `None` if it cannot be understood.
pub fn parse_threshold(input: &str) -> Option<f64> {
    let text = input.trim().to_lowercase().replace(',', ".");
    if text.is_empty() {
        return None;
    }
    if matches!(text.as_str(), "off" | "none" | "0") {
        return Some(0.0);
    }
    let (number, factor) = if let Some(n) = text.strip_suffix("km") {
        (n, 1.0)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 0.001)
    } else {
        (text.as_str(), 1.0)
    };
    let value: f64 = number.trim().parse().ok()?;
    (value >= 0.0 && value.is_finite()).then_some(value * factor)
}

#[cfg(test)]
mod tests;
