//! The live dashboard: terminal setup, the event loop, and what each key does.

pub mod dashboard;
pub mod keys;
pub mod logo;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::DefaultTerminal;
use tokio::time::{MissedTickBehavior, interval};

use self::dashboard::{StravaUiState, Tab, View};
use self::keys::Action;
use crate::model::SessionRecord;
use crate::pad::{MAX_SPEED_KMH, MIN_SPEED_KMH, PadHandle};
use crate::store::SessionStore;
use crate::strava::StravaService;
use crate::strava::config::StravaConfig;
use crate::tracker::SessionTracker;

const REDRAW_INTERVAL: Duration = Duration::from_millis(500);
const SPEED_STEP: f64 = 0.5;
const FINE_SPEED_STEP: f64 = 0.1;
/// Upload threshold steps on the Strava tab, in km: 100 m, or 10 m with Ctrl.
const THRESHOLD_STEP: f64 = 0.1;
const FINE_THRESHOLD_STEP: f64 = 0.01;
const MAX_THRESHOLD_KM: f64 = 42.0;

/// Dashboard state that outlives a single frame.
pub struct App {
    pub tab: Tab,
    pub speed: f64,
    pub strava_state: StravaUiState,
    pub history: Vec<SessionRecord>,
    pub strava: StravaService,
    pub store: SessionStore,
    pub tracker: SessionTracker,
    /// Sessions closed during this run, in order.
    pub recorded: Vec<SessionRecord>,
}

impl App {
    pub fn new(
        start_speed: f64,
        history: Vec<SessionRecord>,
        strava: StravaService,
        store: SessionStore,
    ) -> Self {
        let strava_state = strava_ui_state(&strava, &history);
        Self {
            tab: Tab::default(),
            speed: start_speed,
            strava_state,
            history,
            strava,
            store,
            tracker: SessionTracker::default(),
            recorded: Vec::new(),
        }
    }

    /// Applies a key press. Returns `false` when the user asked to quit.
    pub fn apply(&mut self, action: Action, pad: &PadHandle) -> bool {
        let on_strava_tab = self.tab == Tab::Strava;
        match action {
            Action::Quit => return false,
            Action::NextTab => self.tab = self.tab.next(),
            Action::PreviousTab => self.tab = self.tab.previous(),
            // On the Strava tab, up and down edit the upload threshold instead of the belt
            // speed, so settings can be changed without touching the pad.
            Action::Up { fine } if on_strava_tab => {
                self.adjust_threshold(if fine {
                    FINE_THRESHOLD_STEP
                } else {
                    THRESHOLD_STEP
                });
            }
            Action::Down { fine } if on_strava_tab => {
                self.adjust_threshold(-if fine {
                    FINE_THRESHOLD_STEP
                } else {
                    THRESHOLD_STEP
                });
            }
            Action::Up { fine } => {
                self.change_speed(if fine { FINE_SPEED_STEP } else { SPEED_STEP }, pad);
            }
            Action::Down { fine } => {
                self.change_speed(-if fine { FINE_SPEED_STEP } else { SPEED_STEP }, pad);
            }
            Action::ToggleMute if on_strava_tab => {
                let muted = !self.strava_state.muted;
                if matches!(self.strava.set_hide_from_feed(muted), Ok(true)) {
                    self.strava_state.muted = muted;
                }
            }
            Action::ToggleMute => {}
            Action::ToggleBelt => {
                if pad.session.borrow().is_active() {
                    pad.stop_belt();
                } else {
                    pad.start_belt(self.speed);
                }
            }
        }
        true
    }

    fn change_speed(&mut self, delta: f64, pad: &PadHandle) {
        // Rounded to a tenth so repeated steps never drift.
        let speed = ((self.speed + delta) * 10.0).round() / 10.0;
        self.speed = speed.clamp(MIN_SPEED_KMH, MAX_SPEED_KMH);
        pad.set_speed(self.speed);
    }

    fn adjust_threshold(&mut self, delta_km: f64) {
        // Whole 10 m steps, so repeated floating-point steps never drift.
        let meters = ((self.strava_state.threshold_km + delta_km) * 100.0).round() * 10.0;
        let km = (meters / 1000.0).clamp(0.0, MAX_THRESHOLD_KM);
        if !matches!(self.strava.set_min_upload_distance_km(km), Ok(true)) {
            return;
        }
        self.strava_state = strava_ui_state(&self.strava, &self.history);
    }

    /// Persists a closed session as soon as it closes, so a crash or a closed terminal
    /// cannot lose a walk that already finished.
    pub fn record(&mut self, session: Option<SessionRecord>) {
        let Some(session) = session else { return };
        // Best effort: a failed write must not end the walk.
        let _ = self.store.append(&session);
        self.recorded.push(session);
    }
}

/// Snapshot of the Strava settings and upload backlog for the Strava tab.
pub fn strava_ui_state(service: &StravaService, history: &[SessionRecord]) -> StravaUiState {
    let config = service.config();
    let pending = service.pending(history, false).len();
    StravaUiState {
        connected: config.as_ref().is_some_and(StravaConfig::is_authorized),
        athlete_name: config.as_ref().and_then(|c| c.athlete_name.clone()),
        muted: config.as_ref().is_some_and(|c| c.hide_from_feed),
        threshold_km: config.as_ref().map_or(0.0, |c| c.min_upload_distance_km),
        pending_count: pending,
        below_threshold_count: service.pending(history, true).len() - pending,
    }
}

/// Runs the dashboard until the user quits, feeding status samples to the session tracker
/// along the way.
pub async fn run(terminal: &mut DefaultTerminal, app: &mut App, pad: &mut PadHandle) -> Result<()> {
    let tz = TimeZone::system();
    let mut events = EventStream::new();
    let mut redraw = interval(REDRAW_INTERVAL);
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut samples = pad.session.clone();
    let mut pad_alive = true;

    loop {
        draw(terminal, app, pad, &tz)?;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => {
                    if let Some(action) = keys::action(&key)
                        && !app.apply(action, pad)
                    {
                        return Ok(());
                    }
                }
                // Resizes and everything else just lead to a redraw.
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            changed = samples.changed(), if pad_alive => {
                if changed.is_err() {
                    pad_alive = false;
                    continue;
                }
                let sample = samples.borrow_and_update().clone();
                let closed = app.tracker.on_sample(&sample, Timestamp::now());
                app.record(closed);
            }
            // The pad goes quiet while it idles at unchanging values, so the coalesce
            // window is also checked on a timer.
            _ = redraw.tick() => {
                let closed = app.tracker.tick(Timestamp::now());
                app.record(closed);
            }
        }
    }
}

pub fn draw(
    terminal: &mut DefaultTerminal,
    app: &App,
    pad: &PadHandle,
    tz: &TimeZone,
) -> Result<()> {
    let session = pad.session.borrow().clone();
    let connection = pad.connection.borrow().clone();
    terminal.draw(|frame| {
        dashboard::render(
            frame,
            &View {
                tab: app.tab,
                session: &session,
                connection: &connection,
                strava: &app.strava_state,
                history: &app.history,
                now: Timestamp::now(),
                tz,
            },
        );
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad::{self, PadCommand};
    use crate::store::DataDir;
    use crate::strava::config::{StravaConfig, StravaConfigStore};

    fn app(dir: &DataDir) -> App {
        App::new(2.5, Vec::new(), StravaService::new(dir), dir.sessions())
    }

    #[tokio::test]
    async fn arrow_keys_drive_the_belt_speed_within_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let mut app = app(&dir);
        let (handle, mut pad) = pad::channels();

        app.apply(Action::Up { fine: false }, &handle);
        app.apply(Action::Up { fine: true }, &handle);
        assert_eq!(app.speed, 3.1);
        for _ in 0..20 {
            app.apply(Action::Down { fine: false }, &handle);
        }
        assert_eq!(app.speed, 0.5);

        let mut sent = Vec::new();
        while let Ok(PadCommand::SetSpeed(s)) = pad.commands.try_recv() {
            sent.push(s);
        }
        assert_eq!(&sent[..3], &[3.0, 3.1, 2.6]);
        assert_eq!(*sent.last().unwrap(), 0.5);
    }

    #[tokio::test]
    async fn space_starts_an_idle_belt_and_stops_a_moving_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let mut app = app(&dir);
        let (handle, mut pad) = pad::channels();

        app.apply(Action::ToggleBelt, &handle);
        assert!(matches!(
            pad.commands.try_recv(),
            Ok(PadCommand::StartBelt(2.5))
        ));

        pad.session
            .send_modify(|s| s.state = crate::model::BeltState::Running);
        app.apply(Action::ToggleBelt, &handle);
        assert!(matches!(pad.commands.try_recv(), Ok(PadCommand::StopBelt)));
    }

    #[tokio::test]
    async fn the_strava_tab_edits_and_saves_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        StravaConfigStore::new(&dir)
            .save(&StravaConfig {
                client_id: "id".into(),
                refresh_token: Some("r".into()),
                min_upload_distance_km: 0.3,
                ..Default::default()
            })
            .unwrap();
        let mut app = app(&dir);
        let (handle, mut pad) = pad::channels();

        app.apply(Action::NextTab, &handle);
        app.apply(Action::PreviousTab, &handle);
        app.apply(Action::PreviousTab, &handle);
        assert_eq!(app.tab, Tab::Strava);

        for _ in 0..3 {
            app.apply(Action::Up { fine: false }, &handle);
        }
        app.apply(Action::Down { fine: true }, &handle);
        app.apply(Action::ToggleMute, &handle);
        assert_eq!(app.strava_state.threshold_km, 0.59);
        assert!(app.strava_state.muted);

        let saved = StravaConfigStore::new(&dir).load().unwrap();
        assert_eq!(saved.min_upload_distance_km, 0.59);
        assert!(saved.hide_from_feed);
        // None of it reached the pad.
        assert!(pad.commands.try_recv().is_err());

        for _ in 0..10 {
            app.apply(Action::Down { fine: false }, &handle);
        }
        assert_eq!(app.strava_state.threshold_km, 0.0);
    }

    #[tokio::test]
    async fn closed_sessions_are_saved_immediately() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let mut app = app(&dir);
        let session = crate::pad::demo::sample_history(Timestamp::now()).remove(0);
        app.record(Some(session.clone()));
        app.record(None);
        assert_eq!(app.recorded, vec![session.clone()]);
        assert_eq!(dir.sessions().load(), vec![session]);
    }
}
