//! Strava flows against a mocked API.

use std::time::Duration;

use jiff::Timestamp;
use wiremock::matchers::{body_json, body_string_contains, header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: DataDir,
    server: MockServer,
}

impl Fixture {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        Self {
            _tmp: tmp,
            dir,
            server: MockServer::start().await,
        }
    }

    fn login(&self, config: &StravaConfig) {
        StravaConfigStore::new(&self.dir).save(config).unwrap();
    }

    fn service(&self) -> StravaService {
        let mut service = StravaService::with_endpoints(
            &self.dir,
            Endpoints {
                oauth: format!("{}/oauth", self.server.uri()),
                api: format!("{}/api/v3", self.server.uri()),
            },
        );
        service.api_mut().processing_poll = Duration::from_millis(10);
        service.api_mut().processing_timeout = Duration::from_millis(200);
        service
    }

    async fn mock(&self, mock: Mock) {
        mock.mount(&self.server).await;
    }
}

fn connected() -> StravaConfig {
    StravaConfig {
        client_id: "id".into(),
        client_secret: "secret".into(),
        access_token: Some("valid-token".into()),
        refresh_token: Some("refresh".into()),
        expires_at: Timestamp::now().as_second() + 3600,
        ..Default::default()
    }
}

fn record(km: f64) -> SessionRecord {
    SessionRecord {
        started_at: "2026-11-12T07:30:00.123Z".parse().unwrap(),
        ended_at: "2026-11-12T08:00:00Z".parse().unwrap(),
        duration_seconds: 1800,
        distance_km: km,
        steps: 2300,
        calories: 128,
        avg_speed_kmh: 3.0,
        max_speed_kmh: 3.5,
    }
}

fn upload_request() -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/api/v3/uploads"))
        .and(header("authorization", "Bearer valid-token"))
}

fn upload_accepted(upload_id: i64) -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(serde_json::json!({
        "id": upload_id, "status": "Your activity is still being processed."
    }))
}

fn accepts_upload(upload_id: i64) -> Mock {
    upload_request().respond_with(upload_accepted(upload_id))
}

fn processed(upload_id: i64, activity_id: i64) -> Mock {
    Mock::given(method("GET"))
        .and(path(format!("/api/v3/uploads/{upload_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": upload_id, "activity_id": activity_id
        })))
}

#[tokio::test]
async fn uploads_a_session_and_marks_it_as_a_walk() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(
        upload_request()
            .and(body_string_contains("name=\"data_type\"\r\n\r\ntcx"))
            .and(body_string_contains("name=\"trainer\"\r\n\r\n1"))
            .and(body_string_contains("cli-walker-1794468600"))
            .respond_with(upload_accepted(7))
            .expect(1),
    )
    .await;
    f.mock(processed(7, 99)).await;
    f.mock(
        Mock::given(method("PUT"))
            .and(path("/api/v3/activities/99"))
            .and(body_json(serde_json::json!({ "sport_type": "Walk" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1),
    )
    .await;

    let mut service = f.service();
    let result = service.upload(&record(2.0), false).await;
    assert_eq!(
        result,
        UploadResult::Uploaded {
            activity_id: Some(99),
            warning: None
        }
    );
    assert_eq!(service.activity_id_for(&record(2.0)), Some(99));
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::AlreadyUploaded
    );
    // A fresh service reads the log back from disk.
    assert_eq!(f.service().activity_id_for(&record(2.0)), Some(99));
}

#[tokio::test]
async fn mutes_the_activity_in_the_same_update() {
    let f = Fixture::new().await;
    f.login(&StravaConfig {
        hide_from_feed: true,
        ..connected()
    });
    f.mock(accepts_upload(7)).await;
    f.mock(processed(7, 99)).await;
    f.mock(
        Mock::given(method("PUT"))
            .and(path("/api/v3/activities/99"))
            .and(body_json(
                serde_json::json!({ "sport_type": "Walk", "hide_from_home": true }),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1),
    )
    .await;
    let result = f.service().upload(&record(2.0), false).await;
    assert!(
        matches!(result, UploadResult::Uploaded { warning: None, .. }),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_failed_update_is_a_warning_not_a_failure() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(accepts_upload(7)).await;
    f.mock(processed(7, 99)).await;
    f.mock(
        Mock::given(method("PUT"))
            .and(path("/api/v3/activities/99"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "message": "Record Not Found",
                "errors": [{ "resource": "Activity", "field": "id", "code": "invalid" }]
            }))),
    )
    .await;

    let mut service = f.service();
    match service.upload(&record(2.0), false).await {
        UploadResult::Uploaded {
            activity_id: Some(99),
            warning: Some(w),
        } => assert!(
            w.contains("404") && w.contains("Record Not Found: id invalid"),
            "{w}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::AlreadyUploaded
    );
}

#[tokio::test]
async fn slow_processing_still_counts_as_uploaded() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(accepts_upload(7)).await;
    f.mock(
        Mock::given(method("GET"))
            .and(path("/api/v3/uploads/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 7 }))),
    )
    .await;
    let mut service = f.service();
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::Uploaded {
            activity_id: None,
            warning: None
        }
    );
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::AlreadyUploaded
    );
}

#[tokio::test]
async fn losing_strava_after_acceptance_is_a_warning() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(accepts_upload(7)).await;
    f.mock(
        Mock::given(method("GET"))
            .and(path("/api/v3/uploads/7"))
            .respond_with(ResponseTemplate::new(503)),
    )
    .await;
    let mut service = f.service();
    match service.upload(&record(2.0), false).await {
        UploadResult::Uploaded {
            activity_id: None,
            warning: Some(w),
        } => assert!(w.contains("503"), "{w}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::AlreadyUploaded
    );
}

#[tokio::test]
async fn a_processing_rejection_is_a_failure_and_can_be_retried() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(accepts_upload(7)).await;
    f.mock(
        Mock::given(method("GET"))
            .and(path("/api/v3/uploads/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 7, "error": "Time information is missing"
            }))),
    )
    .await;
    let mut service = f.service();
    match service.upload(&record(2.0), false).await {
        UploadResult::Failed(reason) => assert!(reason.contains("Time information is missing")),
        other => panic!("{other:?}"),
    }
    assert_eq!(service.pending(&[record(2.0)], false).len(), 1);
}

#[tokio::test]
async fn refreshes_an_expired_token_and_keeps_the_rotated_refresh_token() {
    let f = Fixture::new().await;
    f.login(&StravaConfig {
        access_token: Some("stale".into()),
        expires_at: 0,
        ..connected()
    });
    f.mock(
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "valid-token",
                "refresh_token": "rotated",
                "expires_at": 4_000_000_000_i64
            })))
            .expect(1),
    )
    .await;
    f.mock(accepts_upload(7)).await;
    f.mock(processed(7, 99)).await;
    f.mock(Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)))
        .await;

    let result = f.service().upload(&record(2.0), false).await;
    assert!(
        matches!(result, UploadResult::Uploaded { .. }),
        "{result:?}"
    );
    let saved = StravaConfigStore::new(&f.dir).load().unwrap();
    assert_eq!(saved.access_token.as_deref(), Some("valid-token"));
    assert_eq!(saved.refresh_token.as_deref(), Some("rotated"));
    assert_eq!(saved.expires_at, 4_000_000_000);
}

#[tokio::test]
async fn an_unauthorized_upload_suggests_logging_in_again() {
    let f = Fixture::new().await;
    f.login(&connected());
    f.mock(
        Mock::given(method("POST"))
            .and(path("/api/v3/uploads"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "message": "Authorization Error"
            }))),
    )
    .await;
    match f.service().upload(&record(2.0), false).await {
        UploadResult::Failed(reason) => {
            assert!(reason.contains("(401): Authorization Error"), "{reason}");
            assert!(reason.contains("strava login"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn threshold_filters_uploads_and_pending() {
    let f = Fixture::new().await;
    f.login(&StravaConfig {
        min_upload_distance_km: 1.0,
        ..connected()
    });
    let mut service = f.service();
    assert_eq!(
        service.upload(&record(0.5), false).await,
        UploadResult::TooShort {
            distance_km: 0.5,
            threshold_km: 1.0
        }
    );
    assert_eq!(service.pending(&[record(0.5)], false), []);
    assert_eq!(service.pending(&[record(0.5)], true).len(), 1);

    assert!(service.set_min_upload_distance_km(-3.0).unwrap());
    assert_eq!(service.min_upload_distance_km(), 0.0);
}

#[tokio::test]
async fn not_connected_without_a_refresh_token() {
    let f = Fixture::new().await;
    let mut service = f.service();
    assert_eq!(
        service.upload(&record(2.0), false).await,
        UploadResult::NotConnected
    );
    assert!(!service.set_hide_from_feed(true).unwrap());
    assert!(!service.set_min_upload_distance_km(1.0).unwrap());
}

/// Plays the browser: follows the authorize URL's redirect to the loopback listener.
fn browser_redirects_to(port: u16, query: &'static str) {
    tokio::spawn(async move {
        let url = format!("http://127.0.0.1:{port}/callback?{query}");
        let _ = reqwest::get(url).await;
    });
}

#[tokio::test]
async fn logs_in_through_the_loopback_redirect() {
    let f = Fixture::new().await;
    // Settings from an earlier login survive re-authorizing.
    f.login(&StravaConfig {
        hide_from_feed: true,
        min_upload_distance_km: 0.7,
        ..connected()
    });
    f.mock(
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code=the-code"))
            .and(body_string_contains("client_id=new-id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "a", "refresh_token": "r", "expires_at": 4_000_000_000_i64,
                "athlete": { "firstname": "Ada", "lastname": "Walker" }
            }))),
    )
    .await;

    let mut prompted = String::new();
    let result = f
        .service()
        .login(
            "new-id",
            "new-secret",
            LoginOptions {
                port: 0,
                timeout: Duration::from_secs(5),
                open_browser: false,
            },
            |url, port| {
                prompted = url.to_owned();
                browser_redirects_to(
                    port,
                    "state=&code=the-code&scope=read,activity:write,activity:read_all",
                );
            },
        )
        .await;

    assert_eq!(
        result,
        LoginResult::Success {
            athlete_name: Some("Ada Walker".into())
        }
    );
    assert!(prompted.contains("client_id=new-id"), "{prompted}");
    assert!(
        prompted.contains("scope=activity%3Awrite%2Cactivity%3Aread_all"),
        "{prompted}"
    );
    assert!(
        prompted.contains("redirect_uri=http%3A%2F%2Flocalhost%3A"),
        "{prompted}"
    );
    let saved = StravaConfigStore::new(&f.dir).load().unwrap();
    assert_eq!(saved.client_id, "new-id");
    assert_eq!(saved.refresh_token.as_deref(), Some("r"));
    assert!(saved.hide_from_feed);
    assert_eq!(saved.min_upload_distance_km, 0.7);
}

#[tokio::test]
async fn login_fails_when_a_scope_was_unticked() {
    let f = Fixture::new().await;
    let result = f
        .service()
        .login(
            "id",
            "secret",
            LoginOptions {
                port: 0,
                timeout: Duration::from_secs(5),
                open_browser: false,
            },
            |_, port| browser_redirects_to(port, "code=c&scope=read,activity:write"),
        )
        .await;
    assert!(
        matches!(&result, LoginResult::Failure(reason) if reason.contains("activity:read_all")),
        "{result:?}"
    );
}

#[test]
fn formats_and_parses_thresholds() {
    assert_eq!(format_threshold(0.0), "off");
    assert_eq!(format_threshold(0.7), "700 m");
    assert_eq!(format_threshold(1.0), "1 km");
    assert_eq!(format_threshold(1.5), "1.5 km");
    assert_eq!(format_threshold(12.34), "12.34 km");

    assert_eq!(parse_threshold("500m"), Some(0.5));
    assert_eq!(parse_threshold(" 1,5 KM "), Some(1.5));
    assert_eq!(parse_threshold("2"), Some(2.0));
    assert_eq!(parse_threshold("off"), Some(0.0));
    assert_eq!(parse_threshold("none"), Some(0.0));
    assert_eq!(parse_threshold("-1km"), None);
    assert_eq!(parse_threshold("far"), None);
    assert_eq!(parse_threshold(""), None);
}

#[test]
fn names_activities_by_time_of_day() {
    let tz = TimeZone::UTC;
    let at = |hour: u8| SessionRecord {
        started_at: format!("2026-11-12T{hour:02}:15:00Z").parse().unwrap(),
        ..record(1.0)
    };
    assert_eq!(activity_name(&at(4), &tz), "Night Treadmill Walk");
    assert_eq!(activity_name(&at(7), &tz), "Morning Treadmill Walk");
    assert_eq!(activity_name(&at(12), &tz), "Lunch Treadmill Walk");
    assert_eq!(activity_name(&at(15), &tz), "Afternoon Treadmill Walk");
    assert_eq!(activity_name(&at(20), &tz), "Evening Treadmill Walk");
    assert_eq!(activity_name(&at(23), &tz), "Night Treadmill Walk");
}
