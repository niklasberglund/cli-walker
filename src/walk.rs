//! The default command: connect to the pad, show the dashboard, and on exit print a summary of
//! every session recorded and upload them to Strava.

use std::time::Duration;

use crate::paint::Paint;
use anyhow::Result;
use jiff::Timestamp;

use crate::model::{SessionRecord, format_duration, truncate_2};
use crate::pad::{self, ConnectionState};
use crate::store::DataDir;
use crate::strava::{StravaService, UploadResult, format_threshold};
use crate::tui::{self, App};

pub struct WalkOptions {
    pub start_speed: f64,
    pub debug: bool,
    pub no_strava: bool,
    /// Simulated pad and sample data in a throwaway directory.
    pub demo: bool,
}

/// How long quitting waits for the pad to stop the belt and disconnect.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Set by the end-to-end tests. The app starts the belt as soon as it connects, so a test
/// that reached the real pad would run the treadmill unattended.
pub const NO_HARDWARE_ENV: &str = "CLI_WALKER_NO_HARDWARE";

pub async fn walk(options: WalkOptions) -> Result<()> {
    if !options.demo && std::env::var_os(NO_HARDWARE_ENV).is_some() {
        anyhow::bail!("{NO_HARDWARE_ENV} is set, so the real pad is off limits; use --demo");
    }
    let data_dir = if options.demo {
        pad::demo::prepare_data_dir()?
    } else {
        DataDir::default_location()?
    };
    let store = data_dir.sessions();
    let history = store.load();
    let strava = StravaService::new(&data_dir);
    let mut app = App::new(options.start_speed, history, strava, store);

    // Full screen on the alternate screen: it has no scrollback, so a resize cannot pull
    // earlier shell output into view, and the shell comes back untouched on exit. Set up
    // before the pad, which starts the belt on connecting: failing here must not leave it
    // running with nothing to stop it.
    let mut terminal = ratatui::try_init()?;
    let mut pad = if options.demo {
        pad::demo::spawn(options.start_speed)
    } else {
        pad::ble::spawn(&data_dir, options.debug, options.start_speed)
    };
    let outcome = tui::run(&mut terminal, &mut app, &mut pad).await;
    let connection = pad.connection.borrow().clone();
    // Stop a moving belt before the terminal goes back to the shell. The pad drops commands
    // that come too close together, so this can take a moment.
    pad.shutdown(SHUTDOWN_TIMEOUT).await;
    drop(terminal);
    ratatui::try_restore()?;
    println!();

    if let ConnectionState::Error(message) = connection {
        println!("{}", format!("Connection error: {message}").red());
    }
    let closed = app.tracker.finish(Timestamp::now());
    app.record(closed);
    print_summary(&app.recorded);

    if !options.no_strava && !options.demo {
        for session in &app.recorded {
            upload(&mut app.strava, session).await;
        }
    }
    outcome
}

fn print_summary(sessions: &[SessionRecord]) {
    if sessions.is_empty() {
        return;
    }
    let many = sessions.len() > 1;
    println!();
    let title = if many {
        format!("Session Summary ({} sessions)", sessions.len())
    } else {
        "Session Summary".to_owned()
    };
    println!("{}", title.green().bold());
    let indent = if many { "    " } else { "  " };
    for (i, s) in sessions.iter().enumerate() {
        if many {
            println!("  {}", format!("Session {}", i + 1).bold());
        }
        println!("{indent}Distance: {} km", truncate_2(s.distance_km));
        println!("{indent}Steps:    {}", s.steps);
        println!("{indent}Time:     {}", format_duration(s.duration_seconds));
        println!("{indent}Calories: ~{} kcal", s.calories);
    }
    if many {
        let km: f64 = sessions.iter().map(|s| s.distance_km).sum();
        let seconds: u32 = sessions.iter().map(|s| s.duration_seconds).sum();
        let steps: u32 = sessions.iter().map(|s| s.steps).sum();
        println!(
            "  {}",
            format!(
                "Total: {} km, {}, {steps} steps",
                truncate_2(km),
                format_duration(seconds)
            )
            .dim()
        );
    }
}

async fn upload(service: &mut StravaService, record: &SessionRecord) {
    if !service.is_connected() {
        return;
    }
    let threshold = service.min_upload_distance_km();
    if record.distance_km < threshold {
        println!();
        println!(
            "{}{} km is below the {} threshold.",
            "Not uploaded to Strava: ".yellow(),
            truncate_2(record.distance_km),
            format_threshold(threshold)
        );
        println!(
            "{}",
            "  Upload it anyway with `cli-walker strava sync --all`.".dim()
        );
        return;
    }
    println!();
    println!("{}", "Uploading to Strava…".dim());
    match service.upload(record, false).await {
        UploadResult::Uploaded {
            activity_id,
            warning,
        } => {
            match activity_id {
                Some(id) => println!(
                    "{}",
                    format!("Uploaded to Strava: https://www.strava.com/activities/{id}").green()
                ),
                None => println!(
                    "{}{}",
                    "Uploaded to Strava.".green(),
                    " (still processing)".dim()
                ),
            }
            if let Some(w) = warning {
                println!("{}", format!("  ! {w}").yellow());
            }
        }
        UploadResult::Failed(reason) => println!(
            "{}{}",
            format!("Strava upload failed: {reason}").yellow(),
            " (retry with `cli-walker strava sync`)".dim()
        ),
        UploadResult::TooShort {
            distance_km,
            threshold_km,
        } => println!(
            "{}{} km is below the {} threshold.",
            "Not uploaded to Strava: ".yellow(),
            truncate_2(distance_km),
            format_threshold(threshold_km)
        ),
        UploadResult::AlreadyUploaded | UploadResult::NotConnected => {}
    }
}
