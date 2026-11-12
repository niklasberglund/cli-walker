//! The `strava` subcommands, run as a user would, against a throwaway home directory. None of
//! them touch Bluetooth or the network.
//!
//! Unix only: the data directory is found through `HOME`, which Windows does not use.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;

struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn data(&self) -> std::path::PathBuf {
        self.0.path().join(".cli-walker")
    }

    fn write(&self, name: &str, contents: &str) {
        fs::create_dir_all(self.data()).unwrap();
        fs::write(self.data().join(name), contents).unwrap();
    }

    fn logged_in(self) -> Self {
        self.write(
            "strava.json",
            r#"{"clientId":"1","clientSecret":"s","accessToken":"a","refreshToken":"r","expiresAt":4000000000,"athleteName":"Ada Walker"}"#,
        );
        self
    }

    fn with_sessions(self) -> Self {
        self.write(
            "sessions.jsonl",
            concat!(
                r#"{"startedAt":"2026-10-21T19:43:05.512Z","endedAt":"2026-10-21T20:51:22Z","durationSeconds":4097,"distanceKm":4.77,"steps":7353,"calories":250,"avgSpeedKmh":4.2,"maxSpeedKmh":4.8}"#,
                "\n",
                r#"{"startedAt":"2026-10-22T07:00:00Z","endedAt":"2026-10-22T07:05:00Z","durationSeconds":300,"distanceKm":0.25,"steps":380,"calories":18,"avgSpeedKmh":3.0,"maxSpeedKmh":3.0}"#,
                "\n",
            ),
        );
        self
    }

    /// Runs `cli-walker strava <args>` and returns its output, which must be plain text.
    fn strava(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_cli-walker"))
            .arg("strava")
            .args(args)
            .env("HOME", self.0.path())
            .env("CLI_WALKER_NO_HARDWARE", "1")
            .env_remove("STRAVA_CLIENT_ID")
            .env_remove("STRAVA_CLIENT_SECRET")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            !text.contains('\u{1b}'),
            "escape codes in piped output: {text:?}"
        );
        text
    }

    fn config(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.data().join("strava.json")).unwrap()).unwrap()
    }
}

#[test]
fn status_before_login() {
    let home = Home::new();
    assert!(
        home.strava(&["status"])
            .contains("Not connected to Strava.")
    );
    assert!(home.strava(&["threshold", "1km"]).contains("strava login"));
    assert!(home.strava(&["sync"]).contains("Not connected"));
}

#[test]
fn status_counts_pending_sessions_and_the_threshold() {
    let home = Home::new().logged_in().with_sessions();
    let out = home.strava(&["status"]);
    assert!(out.contains("Connected to Strava as Ada Walker."), "{out}");
    assert!(out.contains("2 session(s) waiting to upload"), "{out}");

    let out = home.strava(&["threshold", "500m"]);
    assert!(
        out.contains("Sessions shorter than 500 m will not be uploaded."),
        "{out}"
    );
    assert_eq!(home.config()["minUploadDistanceKm"], 0.5);

    let out = home.strava(&["status"]);
    assert!(out.contains("1 session(s) waiting to upload"), "{out}");
    assert!(
        out.contains("1 session(s) skipped for being below the threshold."),
        "{out}"
    );

    assert!(
        home.strava(&["threshold"])
            .contains("Minimum upload distance is 500 m.")
    );
    assert!(
        home.strava(&["threshold", "far"])
            .contains("Could not read \"far\"")
    );
    home.strava(&["threshold", "off"]);
    assert_eq!(home.config()["minUploadDistanceKm"], 0.0);
}

#[test]
fn mute_toggles_and_reports() {
    let home = Home::new().logged_in();
    assert!(home.strava(&["mute"]).contains("currently visible"));
    assert!(
        home.strava(&["mute", "on"])
            .contains("Uploads will be muted")
    );
    assert_eq!(home.config()["hideFromFeed"], true);
    assert!(home.strava(&["mute"]).contains("currently muted"));
    home.strava(&["mute", "off"]);
    assert_eq!(home.config()["hideFromFeed"], false);
}

#[cfg(unix)]
#[test]
fn settings_keep_the_credentials_file_private() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new().logged_in();
    home.strava(&["mute", "on"]);
    let mode = fs::metadata(home.data().join("strava.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn dry_run_sync_writes_tcx_files_and_uploads_nothing() {
    let home = Home::new().logged_in().with_sessions();
    let out = home.strava(&["sync", "--dry-run", "--limit", "1"]);
    assert!(out.contains("Wrote 1 TCX file(s)"), "{out}");
    let tcx = home.data().join("tcx/cli-walker-1792611785.tcx");
    assert!(
        fs::read_to_string(&tcx)
            .unwrap()
            .contains("<TotalTimeSeconds>4097</TotalTimeSeconds>")
    );
    assert!(!Path::new(&home.data().join("strava-uploads.json")).exists());

    home.strava(&["threshold", "1km"]);
    let out = home.strava(&["sync", "--dry-run"]);
    assert!(out.contains("Wrote 1 TCX file(s)"), "{out}");
    let out = home.strava(&["sync", "--dry-run", "--all"]);
    assert!(out.contains("Wrote 2 TCX file(s)"), "{out}");
}

#[test]
fn sync_skips_sessions_already_uploaded() {
    let home = Home::new().logged_in().with_sessions();
    home.write(
        "strava-uploads.json",
        r#"{"2026-10-21T19:43:05.512Z": 1, "2026-10-22T07:00:00Z": 0}"#,
    );
    let out = home.strava(&["sync"]);
    assert!(out.contains("Nothing to upload"), "{out}");
}

#[test]
fn logout_forgets_the_credentials() {
    let home = Home::new().logged_in();
    assert!(
        home.strava(&["logout"])
            .contains("Stored credentials removed.")
    );
    assert!(!home.data().join("strava.json").exists());
    assert!(home.strava(&["status"]).contains("Not connected"));
}
