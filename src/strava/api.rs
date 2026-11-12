//! Strava's OAuth token and activity upload endpoints.

use std::time::Duration;

use jiff::Timestamp;
use reqwest::multipart::{Form, Part};
use reqwest::{Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::config::{StravaConfig, StravaConfigStore};
use super::tcx::{DEVICE_NAME, build_tcx, file_stem};
use crate::model::{SessionRecord, truncate_1};

/// `activity:write` creates the upload. `PUT /activities/{id}` answers 404 when the token
/// cannot read the activity, and `activity:read` cannot see private ("Only You") activities,
/// so `activity:read_all` is needed too.
pub const SCOPE: &str = "activity:write,activity:read_all";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub oauth: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            oauth: "https://www.strava.com/oauth".into(),
            api: "https://www.strava.com/api/v3".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StravaError(pub String);

type Result<T> = std::result::Result<T, StravaError>;

/// The outcome of an upload. `warning` describes anything that went wrong *after* Strava had
/// already accepted the activity: the upload still counts as done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityUpload {
    /// `None` when Strava accepted the file but did not finish processing it in time. The
    /// activity still arrives, so this is success: retrying would upload a duplicate.
    pub activity_id: Option<i64>,
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    athlete: Option<Athlete>,
}

#[derive(Deserialize)]
struct Athlete {
    firstname: Option<String>,
    lastname: Option<String>,
}

#[derive(Deserialize)]
struct UploadResponse {
    id: i64,
    error: Option<String>,
    activity_id: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorResponse {
    message: Option<String>,
    errors: Vec<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    field: Option<String>,
    code: Option<String>,
}

pub struct StravaApi {
    client: reqwest::Client,
    endpoints: Endpoints,
    store: StravaConfigStore,
    /// How long to wait for Strava to turn an upload into an activity, and how often to ask.
    pub processing_timeout: Duration,
    pub processing_poll: Duration,
}

impl StravaApi {
    pub fn new(store: StravaConfigStore, endpoints: Endpoints) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("cli-walker/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("HTTP client"),
            endpoints,
            store,
            processing_timeout: Duration::from_secs(90),
            processing_poll: Duration::from_secs(2),
        }
    }

    /// Exchanges an authorization code for tokens and saves them.
    pub async fn exchange_code(&self, config: &StravaConfig, code: &str) -> Result<StravaConfig> {
        let response = self
            .client
            .post(format!("{}/token", self.endpoints.oauth))
            .form(&[
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(network)?;
        let tokens: TokenResponse = decode(response, "token exchange").await?;
        let name = tokens.athlete.map(|a| {
            [a.firstname, a.lastname]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
        });
        let updated = StravaConfig {
            access_token: Some(tokens.access_token),
            refresh_token: Some(tokens.refresh_token),
            expires_at: tokens.expires_at,
            athlete_name: name.filter(|n| !n.trim().is_empty()),
            ..config.clone()
        };
        self.save(&updated)?;
        Ok(updated)
    }

    /// A valid access token, refreshed first if it is expired or nearly so. Strava rotates
    /// refresh tokens, so whatever comes back is saved, including the refresh token.
    pub async fn valid_access_token(
        &self,
        config: &StravaConfig,
    ) -> Result<(String, StravaConfig)> {
        let Some(refresh_token) = &config.refresh_token else {
            return Err(StravaError(
                "Not connected to Strava. Run `cli-walker strava login` first.".into(),
            ));
        };
        if let Some(token) = &config.access_token
            && config.expires_at - 60 > Timestamp::now().as_second()
        {
            return Ok((token.clone(), config.clone()));
        }

        let response = self
            .client
            .post(format!("{}/token", self.endpoints.oauth))
            .form(&[
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(network)?;
        let tokens: TokenResponse = decode(response, "token refresh").await?;
        let updated = StravaConfig {
            access_token: Some(tokens.access_token.clone()),
            refresh_token: Some(tokens.refresh_token),
            expires_at: tokens.expires_at,
            ..config.clone()
        };
        self.save(&updated)?;
        Ok((tokens.access_token, updated))
    }

    /// Uploads a session as a TCX file and waits for the resulting activity.
    ///
    /// Uploading, rather than creating a manual activity, is what gives the activity an
    /// `upload_id` and app attribution. The cost is that it is asynchronous: Strava accepts
    /// the file, then processes it into an activity we have to wait for.
    pub async fn upload_activity(
        &self,
        config: &StravaConfig,
        record: &SessionRecord,
        name: &str,
    ) -> Result<ActivityUpload> {
        let (token, config) = self.valid_access_token(config).await?;
        let external_id = file_stem(record);
        let file = Part::bytes(build_tcx(record).into_bytes())
            .file_name(format!("{external_id}.tcx"))
            .mime_str("application/xml")
            .expect("valid MIME type");
        let form = Form::new()
            .part("file", file)
            .text("data_type", "tcx")
            .text("name", name.to_owned())
            .text("description", describe(record))
            .text("trainer", "1")
            .text("external_id", external_id);
        let response = self
            .client
            .post(format!("{}/uploads", self.endpoints.api))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .map_err(network)?;
        let upload: UploadResponse = decode(response, "activity upload").await?;
        if let Some(error) = upload.error {
            return Err(StravaError(format!("Strava rejected the upload: {error}")));
        }

        // Past this point Strava has the file. A rejection while processing is still an
        // error, but anything else that fails from here is a warning: reporting failure
        // would upload the session again as a duplicate.
        let activity_id = match self.await_processing(&token, upload.id).await {
            Ok(id) => id,
            Err(Processing::Rejected(e)) => return Err(e),
            Err(Processing::Unreachable(e)) => {
                return Ok(ActivityUpload {
                    activity_id: None,
                    warning: Some(format!("uploaded, but could not check on processing: {e}")),
                });
            }
        };

        let mut warning = None;
        if let Some(id) = activity_id {
            // TCX has no Walk sport, so the activity lands as a generic workout until fixed.
            if let Err(e) = self
                .finish_activity(&token, id, config.hide_from_feed)
                .await
            {
                warning = Some(format!(
                    "uploaded, but could not set the activity type: {e}"
                ));
            }
        }
        Ok(ActivityUpload {
            activity_id,
            warning,
        })
    }

    /// Polls the upload until Strava turns it into an activity. `Ok(None)` means it was still
    /// processing when we gave up; the activity is on its way.
    async fn await_processing(
        &self,
        token: &str,
        upload_id: i64,
    ) -> std::result::Result<Option<i64>, Processing> {
        let started = tokio::time::Instant::now();
        while started.elapsed() < self.processing_timeout {
            tokio::time::sleep(self.processing_poll).await;
            let response = self
                .client
                .get(format!("{}/uploads/{upload_id}", self.endpoints.api))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| Processing::Unreachable(network(e)))?;
            let upload: UploadResponse = decode(response, "upload status")
                .await
                .map_err(Processing::Unreachable)?;
            if let Some(error) = upload.error {
                return Err(Processing::Rejected(StravaError(format!(
                    "Strava rejected the upload: {error}"
                ))));
            }
            if let Some(id) = upload.activity_id {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Corrects the processed activity: TCX cannot express a walk, and muting is only
    /// available on update. Both fit in one call.
    async fn finish_activity(
        &self,
        token: &str,
        activity_id: i64,
        hide_from_feed: bool,
    ) -> Result<()> {
        let mut fields = serde_json::json!({ "sport_type": "Walk" });
        if hide_from_feed {
            fields["hide_from_home"] = true.into();
        }
        let response = self
            .client
            .put(format!("{}/activities/{activity_id}", self.endpoints.api))
            .bearer_auth(token)
            .json(&fields)
            .send()
            .await
            .map_err(network)?;
        require_success(response, "setting activity type").await?;
        Ok(())
    }

    fn save(&self, config: &StravaConfig) -> Result<()> {
        self.store
            .save(config)
            .map_err(|e| StravaError(format!("could not save the Strava login: {e:#}")))
    }
}

enum Processing {
    Rejected(StravaError),
    Unreachable(StravaError),
}

fn describe(record: &SessionRecord) -> String {
    format!(
        "{DEVICE_NAME} · {} steps · avg {} km/h · max {} km/h",
        record.steps,
        truncate_1(record.avg_speed_kmh),
        truncate_1(record.max_speed_kmh)
    )
}

fn network(e: reqwest::Error) -> StravaError {
    StravaError(format!("could not reach Strava: {e}"))
}

async fn decode<T: DeserializeOwned>(response: Response, what: &str) -> Result<T> {
    let body = require_success(response, what).await?;
    serde_json::from_str(&body).map_err(|e| {
        StravaError(format!(
            "Strava {what} returned an unexpected response: {e}"
        ))
    })
}

async fn require_success(response: Response, what: &str) -> Result<String> {
    let status = response.status();
    let body = response.text().await.map_err(network)?;
    if status.is_success() {
        return Ok(body);
    }
    let detail = serde_json::from_str::<ErrorResponse>(&body)
        .ok()
        .map(|error| {
            let fields: Vec<String> = error
                .errors
                .iter()
                .filter_map(|d| {
                    let field = d.field.as_ref()?;
                    Some(
                        format!("{field} {}", d.code.as_deref().unwrap_or(""))
                            .trim()
                            .to_owned(),
                    )
                })
                .collect();
            [
                error.message,
                (!fields.is_empty()).then(|| fields.join(", ")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(": ")
        })
        .filter(|d| !d.trim().is_empty())
        .unwrap_or(body);
    let hint = match status {
        StatusCode::UNAUTHORIZED => " — try `cli-walker strava login` again",
        StatusCode::TOO_MANY_REQUESTS => " — Strava rate limit reached, try again later",
        _ => "",
    };
    Err(StravaError(format!(
        "Strava {what} failed ({}): {detail}{hint}",
        status.as_u16()
    )))
}
