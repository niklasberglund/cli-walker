//! `cli-walker strava …`: log in, check status, sync, and change upload settings.

use std::io::{self, BufRead, Write};

use crate::paint::Paint;
use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use jiff::tz::TimeZone;

use crate::model::SessionRecord;
use crate::store::DataDir;
use crate::strava::config::StravaConfig;
use crate::strava::{
    DEFAULT_CALLBACK_PORT, LoginOptions, LoginResult, StravaService, UploadResult, activity_name,
    format_threshold, parse_threshold, tcx,
};

#[derive(Debug, Subcommand)]
pub enum StravaCommand {
    /// Authorize cli-walker against your Strava API application
    Login {
        #[arg(long, env = "STRAVA_CLIENT_ID")]
        client_id: Option<String>,
        #[arg(long, env = "STRAVA_CLIENT_SECRET", hide_env_values = true)]
        client_secret: Option<String>,
        /// Local port for the OAuth redirect
        #[arg(long, default_value_t = DEFAULT_CALLBACK_PORT)]
        port: u16,
        /// Print the authorize URL instead of opening a browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Show the Strava connection and how many sessions are waiting to upload
    Status,
    /// Upload every recorded session that has not reached Strava yet
    Sync {
        /// Only upload the N oldest pending sessions
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Write the TCX files that would be uploaded, and stop
        #[arg(long)]
        dry_run: bool,
        /// Include sessions below the minimum upload distance
        #[arg(long)]
        all: bool,
    },
    /// Keep uploaded activities out of your followers' feeds
    #[command(
        after_help = "Muting is not the same as private. Strava's API cannot set activity \
        visibility; for that, set Privacy Controls > Activities to \"Only You\" at \
        https://www.strava.com/settings/privacy"
    )]
    Mute { state: Option<OnOff> },
    /// Set the minimum session distance that gets uploaded to Strava
    #[command(
        after_help = "Examples: `strava threshold 500m`, `strava threshold 1.5km`, \
        `strava threshold 2` (km), `strava threshold off`"
    )]
    Threshold {
        /// Distance such as 500m, 1.5km, or off
        value: Option<String>,
    },
    /// Forget the stored Strava credentials and tokens
    Logout,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

pub async fn run(command: StravaCommand) -> Result<()> {
    let dir = DataDir::default_location()?;
    let mut service = StravaService::new(&dir);
    match command {
        StravaCommand::Login {
            client_id,
            client_secret,
            port,
            no_browser,
        } => login(&service, client_id, client_secret, port, no_browser).await,
        StravaCommand::Status => {
            status(&service, &dir);
            Ok(())
        }
        StravaCommand::Sync {
            limit,
            dry_run,
            all,
        } => sync(&mut service, &dir, limit, dry_run, all).await,
        StravaCommand::Mute { state } => mute(&service, state),
        StravaCommand::Threshold { value } => threshold(&service, value),
        StravaCommand::Logout => {
            service.logout();
            println!("Disconnected from Strava. Stored credentials removed.");
            Ok(())
        }
    }
}

fn not_connected() {
    println!(
        "{}",
        format!(
            "Not connected to Strava. Run {} first.",
            "cli-walker strava login".bold()
        )
        .yellow()
    );
}

async fn login(
    service: &StravaService,
    client_id: Option<String>,
    client_secret: Option<String>,
    port: u16,
    no_browser: bool,
) -> Result<()> {
    let existing = service.config();
    let client_id = match client_id.or_else(|| existing.as_ref().map(|c| c.client_id.clone())) {
        Some(id) if !id.trim().is_empty() => id,
        _ => prompt("Strava Client ID: ")?,
    };
    let client_secret =
        match client_secret.or_else(|| existing.as_ref().map(|c| c.client_secret.clone())) {
            Some(secret) if !secret.trim().is_empty() => secret,
            _ => rpassword::prompt_password("Strava Client Secret: ")?,
        };
    let (client_id, client_secret) = (client_id.trim(), client_secret.trim());

    if client_id.is_empty() || client_secret.is_empty() {
        println!("{}", "Client ID and Client Secret are required.".red());
        println!(
            "Register an application at {} and set the callback domain to {}.",
            "https://www.strava.com/settings/api".bold(),
            "localhost".bold()
        );
        return Ok(());
    }

    println!();
    if !no_browser {
        println!("Opening your browser to authorize with Strava…");
    }
    let result = service
        .login(
            client_id,
            client_secret,
            LoginOptions {
                port,
                open_browser: !no_browser,
                ..Default::default()
            },
            |url, port| {
                println!(
                    "{}",
                    format!("Waiting for the redirect on http://localhost:{port}/callback").dim()
                );
                if no_browser {
                    println!("Open this URL to authorize:");
                } else {
                    println!("{}", "If the browser does not open, visit:".dim());
                }
                println!("{}", url.dim());
            },
        )
        .await;

    println!();
    match result {
        LoginResult::Success { athlete_name } => {
            let who = athlete_name.map_or_default(|n| format!(" as {}", n.bold()));
            println!("{}", format!("Connected to Strava{who}.").green());
            println!(
                "Finished sessions will upload automatically. Run {} to send past ones.",
                "cli-walker strava sync".bold()
            );
        }
        LoginResult::Failure(reason) => {
            println!("{}", format!("Login failed: {reason}").red());
            if reason.contains("in use") {
                println!("Try a different port with {}.", "--port".bold());
            }
        }
    }
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn status(service: &StravaService, dir: &DataDir) {
    let Some(config) = service.config().filter(StravaConfig::is_authorized) else {
        println!("{}", "Not connected to Strava.".yellow());
        println!("Run {} to connect.", "cli-walker strava login".bold());
        return;
    };
    let who = config.athlete_name.as_deref().unwrap_or("unknown athlete");
    println!(
        "{}",
        format!("Connected to Strava as {}.", who.bold()).green()
    );
    if config.hide_from_feed {
        println!(
            "Uploads are {} — hidden from followers' feeds, still on your profile.",
            "muted".bold()
        );
    } else {
        println!(
            "Uploads are {} in followers' feeds. Mute them with {}.",
            "visible".bold(),
            "cli-walker strava mute on".bold()
        );
    }
    if config.min_upload_distance_km > 0.0 {
        println!(
            "Only sessions of at least {} are uploaded.",
            format_threshold(config.min_upload_distance_km).bold()
        );
    } else {
        println!(
            "Every session is uploaded, however short. Set a minimum with {}.",
            "cli-walker strava threshold 500m".bold()
        );
    }

    let history = dir.sessions().load();
    let pending = service.pending(&history, false).len();
    if pending == 0 {
        println!("All sessions have been uploaded.");
    } else {
        println!(
            "{} session(s) waiting to upload — run {}.",
            pending.to_string().bold(),
            "cli-walker strava sync".bold()
        );
    }
    let too_short = service.pending(&history, true).len() - pending;
    if too_short > 0 {
        println!(
            "{}",
            format!("{too_short} session(s) skipped for being below the threshold.").dim()
        );
    }
}

async fn sync(
    service: &mut StravaService,
    dir: &DataDir,
    limit: Option<usize>,
    dry_run: bool,
    all: bool,
) -> Result<()> {
    if !service.is_connected() {
        not_connected();
        return Ok(());
    }
    let history = dir.sessions().load();
    let mut pending = service.pending(&history, all);
    if let Some(limit) = limit {
        pending.truncate(limit);
    }
    if pending.is_empty() {
        println!("{}", "Nothing to upload — Strava is up to date.".green());
        let threshold = service.min_upload_distance_km();
        if !all && threshold > 0.0 {
            let skipped = service.pending(&history, true).len();
            if skipped > 0 {
                println!(
                    "{}",
                    format!(
                        "{skipped} session(s) are below the {} threshold — upload them with {}.",
                        format_threshold(threshold),
                        "--all".bold()
                    )
                    .dim()
                );
            }
        }
        return Ok(());
    }

    if dry_run {
        let out = tcx::write_tcx_files(dir, &pending)?;
        println!(
            "Wrote {} TCX file(s) to {} — nothing was uploaded.",
            pending.len(),
            out.display().to_string().bold()
        );
        return Ok(());
    }

    println!("Uploading {} session(s) to Strava…", pending.len());
    let tz = TimeZone::system();
    let mut uploaded = 0;
    for record in &pending {
        let label = session_label(record, &tz);
        match service.upload(record, all).await {
            UploadResult::Uploaded {
                activity_id,
                warning,
            } => {
                uploaded += 1;
                let place = activity_id.map_or_else(
                    || "Strava (no activity id returned)".to_owned(),
                    |id| format!("strava.com/activities/{id}"),
                );
                println!("{}", format!("  ✓ {label} → {place}").green());
                if let Some(w) = warning {
                    println!("{}", format!("    ! {w}").yellow());
                }
            }
            UploadResult::AlreadyUploaded => {
                println!("{}", format!("  · {label} (already uploaded)").dim());
            }
            UploadResult::TooShort { threshold_km, .. } => println!(
                "{}",
                format!(
                    "  · {label} (below the {} threshold)",
                    format_threshold(threshold_km)
                )
                .dim()
            ),
            UploadResult::NotConnected => {
                println!(
                    "{}",
                    "  ✗ Lost the Strava connection. Run login again.".red()
                );
                break;
            }
            UploadResult::Failed(reason) => {
                println!("{}", format!("  ✗ {label} — {reason}").red());
                // Rate limits and auth failures will hit every remaining session too.
                if reason.contains("rate limit") || reason.contains("login") {
                    break;
                }
            }
        }
    }
    println!();
    println!("Uploaded {uploaded} of {}.", pending.len());
    Ok(())
}

fn mute(service: &StravaService, state: Option<OnOff>) -> Result<()> {
    let Some(config) = service.config() else {
        not_connected();
        return Ok(());
    };
    let Some(state) = state else {
        let current = if config.hide_from_feed {
            "muted"
        } else {
            "visible"
        };
        println!("Uploads are currently {}.", current.bold());
        return Ok(());
    };
    let hide = matches!(state, OnOff::On);
    service.set_hide_from_feed(hide)?;
    if hide {
        println!(
            "{}",
            "Uploads will be muted — they stay off your followers' feeds.".green()
        );
        println!(
            "{}",
            "This is not the same as private: muted activities are still visible on your profile."
                .dim()
        );
        println!(
            "{}",
            "For real privacy set Privacy Controls > Activities to \"Only You\" at https://www.strava.com/settings/privacy".dim()
        );
    } else {
        println!(
            "{}",
            "Uploads will appear in followers' feeds again.".green()
        );
    }
    Ok(())
}

fn threshold(service: &StravaService, value: Option<String>) -> Result<()> {
    let Some(config) = service.config() else {
        not_connected();
        return Ok(());
    };
    let Some(raw) = value else {
        if config.min_upload_distance_km > 0.0 {
            println!(
                "Minimum upload distance is {}.",
                format_threshold(config.min_upload_distance_km).bold()
            );
        } else {
            println!("No minimum upload distance — every session is uploaded.");
        }
        return Ok(());
    };
    let Some(km) = parse_threshold(&raw) else {
        println!(
            "{}",
            format!("Could not read \"{raw}\" as a distance. Try 500m, 1.5km, or off.").red()
        );
        return Ok(());
    };
    service.set_min_upload_distance_km(km)?;
    if km <= 0.0 {
        println!(
            "{}",
            "Minimum upload distance removed — every session will be uploaded.".green()
        );
    } else {
        println!(
            "{}",
            format!(
                "Sessions shorter than {} will not be uploaded.",
                format_threshold(km).bold()
            )
            .green()
        );
    }
    Ok(())
}

/// `2026-10-21 21:43 · 4.77 km · Evening Treadmill Walk`
fn session_label(record: &SessionRecord, tz: &TimeZone) -> String {
    let local = record.started_at.to_zoned(tz.clone());
    let km = (record.distance_km * 100.0).trunc() / 100.0;
    format!(
        "{} · {km} km · {}",
        local.strftime("%Y-%m-%d %H:%M"),
        activity_name(record, tz)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_sessions_for_the_sync_log() {
        let record = SessionRecord {
            started_at: "2026-10-21T19:43:05Z".parse().unwrap(),
            ended_at: "2026-10-21T20:51:22Z".parse().unwrap(),
            duration_seconds: 4097,
            distance_km: 4.779,
            steps: 7353,
            calories: 250,
            avg_speed_kmh: 4.2,
            max_speed_kmh: 4.8,
        };
        assert_eq!(
            session_label(&record, &TimeZone::UTC),
            "2026-10-21 19:43 · 4.77 km · Evening Treadmill Walk"
        );
    }
}
