//! Draws the dashboard: the logo, then a card with a tab bar, rows of values and a key hint.
//!
//! Rendering is a pure function of `View`, so every tab can be snapshot-tested without a
//! terminal or a pad.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};

use super::logo;
use crate::model::{
    BeltState, SessionRecord, WalkingSession, format_duration, truncate_1, truncate_2,
};
use crate::pad::ConnectionState;
use crate::strava::format_threshold;

/// Every tab is drawn this wide (or the terminal's width, if narrower), so switching tabs
/// never resizes the box.
pub const CARD_WIDTH: u16 = 88;
/// Width of the label column; values start after it.
const LABEL_WIDTH: u16 = 16;
/// Rows above the card: a blank line, the logo, a blank line.
const CARD_TOP: u16 = 1 + logo::HEIGHT + 1;

const SPEED_BAR_WIDTH: usize = 20;
const MAX_SPEED_KMH: f64 = 6.0;

const GRAY: Color = Color::DarkGray;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Session,
    Statistics,
    Strava,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Session, Tab::Statistics, Tab::Strava];

    fn label(self) -> &'static str {
        match self {
            Tab::Session => "Session",
            Tab::Statistics => "Statistics",
            Tab::Strava => "Strava",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Everything the Strava tab shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StravaUiState {
    pub connected: bool,
    pub athlete_name: Option<String>,
    pub muted: bool,
    pub threshold_km: f64,
    pub pending_count: usize,
    pub below_threshold_count: usize,
}

pub struct View<'a> {
    pub tab: Tab,
    pub session: &'a WalkingSession,
    pub connection: &'a ConnectionState,
    pub strava: &'a StravaUiState,
    pub history: &'a [SessionRecord],
    pub now: Timestamp,
    pub tz: &'a TimeZone,
}

enum Row {
    Pair(Line<'static>, Line<'static>),
    Full(Line<'static>),
}

fn pair(label: &str, value: Line<'static>) -> Row {
    Row::Pair(Line::from(label.to_owned()).fg(GRAY), value)
}

pub fn render(frame: &mut Frame, view: &View) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    for (i, line) in logo::lines().into_iter().enumerate() {
        put(buf, area, area.x, area.y + 1 + i as u16, &line, area.width);
    }

    let (title, rows, footer) = match view.tab {
        Tab::Session => session_tab(view),
        Tab::Statistics => statistics_tab(view),
        Tab::Strava => strava_tab(view),
    };
    let card = Rect {
        x: area.x,
        y: area.y + CARD_TOP,
        width: CARD_WIDTH.min(area.width),
        height: area.height.saturating_sub(CARD_TOP),
    };
    draw_card(buf, card, &[tab_bar(view.tab), title], &rows, &footer);
}

fn tab_bar(active: Tab) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let label = format!(" {} ", tab.label());
        spans.push(if tab == active {
            Span::styled(label, Style::new().fg(Color::Black).bg(Color::LightCyan))
        } else {
            Span::styled(label, Style::new().fg(GRAY))
        });
    }
    Line::from(spans)
}

fn heading(text: &str) -> Line<'static> {
    Line::from(text.to_owned()).light_cyan().bold()
}

fn hint(text: &str) -> Line<'static> {
    Line::from(text.to_owned()).fg(GRAY)
}

fn session_tab(view: &View) -> (Line<'static>, Vec<Row>, Line<'static>) {
    let s = view.session;
    // The last sample outlives the link, so belt and speed would go on reporting a pad that
    // may have stopped. Distance, steps and time still describe the walk and stay.
    let live = *view.connection == ConnectionState::Connected;
    let rows = vec![
        pair("Status", connection_line(view.connection)),
        pair("Belt", if live { belt_line(s) } else { unknown() }),
        pair("Speed", if live { speed_line(s) } else { unknown() }),
        pair("Distance", Line::from(s.formatted_distance()).green()),
        pair("Steps", Line::from(s.steps.to_string()).magenta()),
        pair("Time", time_line(s)),
        pair(
            "Calories",
            Line::from(format!("~{} kcal", s.calories_estimate())).yellow(),
        ),
    ];
    (
        heading("CLI-Walker - Kingsmith WalkingPad Monitor"),
        rows,
        hint("Up/Down +/-0.5  Ctrl+Up/Down +/-0.1  Space pause/resume  Tab/Arrows tabs  q quit"),
    )
}

fn connection_line(state: &ConnectionState) -> Line<'static> {
    match state {
        ConnectionState::Disconnected => Line::from("Disconnected").red(),
        ConnectionState::Scanning => Line::from("Scanning...").yellow(),
        ConnectionState::Connecting => Line::from("Connecting...").yellow(),
        ConnectionState::Connected => Line::from("Connected").green(),
        ConnectionState::Error(message) => Line::from(vec![
            Span::styled("Error", Style::new().fg(Color::LightRed)),
            Span::styled(format!("  {message}"), Style::new().fg(GRAY)),
        ]),
    }
}

fn unknown() -> Line<'static> {
    Line::from("—").fg(GRAY)
}

fn belt_line(s: &WalkingSession) -> Line<'static> {
    match s.state {
        BeltState::Running => Line::from("Running").green(),
        BeltState::Starting | BeltState::Accelerating => Line::from("Starting...").yellow(),
        BeltState::Ramping => Line::from("Ramping").yellow(),
        BeltState::Standby => Line::from("Standby").yellow(),
        BeltState::Idle => Line::from("Idle").fg(GRAY),
        BeltState::Unknown => Line::from(format!("Unknown(0x{:02x})", s.raw_state)).magenta(),
    }
}

fn speed_line(s: &WalkingSession) -> Line<'static> {
    let filled = ((s.speed / MAX_SPEED_KMH) * SPEED_BAR_WIDTH as f64)
        .clamp(0.0, SPEED_BAR_WIDTH as f64) as usize;
    let bar_color = if s.speed < 2.0 {
        Color::Green
    } else if s.speed < 4.0 {
        Color::Yellow
    } else {
        Color::Red
    };
    Line::from(vec![
        Span::styled(s.formatted_speed(), Style::new().fg(Color::Cyan)),
        Span::raw(" "),
        Span::styled("█".repeat(filled), Style::new().fg(bar_color)),
        Span::styled("░".repeat(SPEED_BAR_WIDTH - filled), Style::new().fg(GRAY)),
    ])
}

/// The auto-rest warning shares the Time row so the dashboard never changes height.
fn time_line(s: &WalkingSession) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format_duration(s.elapsed_seconds),
        Style::new().fg(Color::Blue),
    )];
    if let Some(remaining) = s.seconds_until_auto_stop() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("Pad auto-rests in {}", format_duration(remaining)),
            Style::new()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// A personal best: a star, the value in its colour, and a gray detail after it.
fn starred(value: String, color: Color, detail: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("★ ", Style::new().fg(Color::Yellow)),
        Span::styled(value, Style::new().fg(color)),
        Span::styled(format!("  {detail}"), Style::new().fg(GRAY)),
    ])
}

fn statistics_tab(view: &View) -> (Line<'static>, Vec<Row>, Line<'static>) {
    let history = view.history;
    let mut rows = Vec::new();
    if history.is_empty() {
        rows.push(Row::Full(hint("No sessions recorded yet - go for a walk!")));
    } else {
        rows.push(pair("Totals", totals_line(history.iter())));
        let now = view.now.to_zoned(view.tz.clone());
        let this_month: Vec<&SessionRecord> = history
            .iter()
            .filter(|r| {
                let at = r.started_at.to_zoned(view.tz.clone());
                at.year() == now.year() && at.month() == now.month()
            })
            .collect();
        rows.push(pair(
            "This Month",
            if this_month.is_empty() {
                hint("No sessions yet")
            } else {
                totals_line(this_month.into_iter())
            },
        ));

        let best = |key: fn(&SessionRecord) -> f64| {
            history
                .iter()
                .reduce(|a, b| if key(b) > key(a) { b } else { a })
                .expect("history is not empty")
        };
        let longest = best(|r| r.distance_km);
        let longest_time = best(|r| f64::from(r.duration_seconds));
        let fastest_avg = best(|r| r.avg_speed_kmh);
        let top_speed = best(|r| r.max_speed_kmh);
        let most_steps = best(|r| f64::from(r.steps));
        let date = |r: &SessionRecord| format_date(r.started_at, view.tz);
        rows.push(pair(
            "Best distance",
            starred(
                format!("{} km", truncate_2(longest.distance_km)),
                Color::Green,
                &date(longest),
            ),
        ));
        rows.push(pair(
            "Best duration",
            starred(
                format_duration(longest_time.duration_seconds),
                Color::Blue,
                &date(longest_time),
            ),
        ));
        rows.push(pair(
            "Top speed",
            starred(
                format!("{} km/h", truncate_1(top_speed.max_speed_kmh)),
                Color::Cyan,
                &format!("avg {} km/h", truncate_1(fastest_avg.avg_speed_kmh)),
            ),
        ));
        rows.push(pair(
            "Most steps",
            starred(
                most_steps.steps.to_string(),
                Color::Magenta,
                &date(most_steps),
            ),
        ));

        rows.push(Row::Full(heading("Recent Sessions")));
        for r in history.iter().rev().take(5) {
            rows.push(Row::Pair(
                Line::from(date(r)).fg(GRAY),
                Line::from(vec![
                    Span::styled(
                        format_duration(r.duration_seconds),
                        Style::new().fg(Color::Blue),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{} km", truncate_2(r.distance_km)),
                        Style::new().fg(Color::Green),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{} steps", r.steps),
                        Style::new().fg(Color::Magenta),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{} km/h avg", truncate_1(r.avg_speed_kmh)),
                        Style::new().fg(Color::Cyan),
                    ),
                ]),
            ));
        }
    }
    (heading("Statistics"), rows, hint("Tab/Arrows tabs  q quit"))
}

fn totals_line<'a>(sessions: impl Iterator<Item = &'a SessionRecord>) -> Line<'static> {
    let (mut count, mut km, mut steps, mut seconds, mut kcal) = (0, 0.0, 0, 0, 0);
    for r in sessions {
        count += 1;
        km += r.distance_km;
        steps += r.steps;
        seconds += r.duration_seconds;
        kcal += r.calories;
    }
    Line::from(vec![
        Span::styled(format!("{count} sessions"), Style::new().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(
            format!("{} km", truncate_2(km)),
            Style::new().fg(Color::Green),
        ),
        Span::raw("  "),
        Span::styled(format!("{steps} steps"), Style::new().fg(Color::Magenta)),
        Span::raw("  "),
        Span::styled(format_duration(seconds), Style::new().fg(Color::Blue)),
        Span::raw("  "),
        Span::styled(format!("~{kcal} kcal"), Style::new().fg(Color::Yellow)),
    ])
}

fn strava_tab(view: &View) -> (Line<'static>, Vec<Row>, Line<'static>) {
    let s = view.strava;
    let gray = |text: String| Span::styled(text, Style::new().fg(GRAY));
    let mut rows = vec![
        pair(
            "Account",
            if s.connected {
                Line::from(vec![
                    Span::styled("Connected", Style::new().fg(Color::Green)),
                    gray(format!(
                        "  {}",
                        s.athlete_name.as_deref().unwrap_or("unknown athlete")
                    )),
                ])
            } else {
                Line::from(vec![
                    Span::styled("Not connected", Style::new().fg(Color::Yellow)),
                    gray("  run `cli-walker strava login`".into()),
                ])
            },
        ),
        pair(
            "Feed",
            if s.muted {
                Line::from(vec![
                    Span::styled("Muted", Style::new().fg(Color::Yellow)),
                    gray("  hidden from followers' feeds".into()),
                ])
            } else {
                Line::from(vec![
                    Span::styled("Visible", Style::new().fg(Color::Green)),
                    gray("  shown in followers' feeds".into()),
                ])
            },
        ),
        pair(
            "Min distance",
            Line::from(vec![
                // Padded so the hint does not shift while the threshold is adjusted.
                Span::styled(
                    format!("{:<8}", format_threshold(s.threshold_km)),
                    Style::new().fg(Color::Cyan),
                ),
                gray("  shorter sessions are not uploaded".into()),
            ]),
        ),
        pair(
            "Pending",
            if s.pending_count == 0 {
                hint("Nothing waiting to upload")
            } else {
                Line::from(format!("{} session(s) waiting", s.pending_count)).magenta()
            },
        ),
    ];
    if s.below_threshold_count > 0 {
        rows.push(pair(
            "Below min",
            hint(&format!(
                "{} session(s) skipped by the threshold",
                s.below_threshold_count
            )),
        ));
    }
    (
        heading("Strava"),
        rows,
        hint("Up/Down +/-100 m  Ctrl+Up/Down +/-10 m  m mute/unmute  Tab/Arrows tabs  q quit"),
    )
}

/// `Oct 21 21:43`, in local time.
fn format_date(at: Timestamp, tz: &TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%b %d %H:%M").to_string()
}

/// Header box, rows separated by rules, footer box. Borders and rules carry the logo's
/// gradient from left to right.
fn draw_card(buf: &mut Buffer, card: Rect, header: &[Line], rows: &[Row], footer: &Line) {
    let width = card.width;
    if width < 4 {
        return;
    }
    let inner = width - 4;
    let mut y = card.y;

    border_row(buf, card, y, '╭', '─', '╮');
    y += 1;
    for line in header {
        boxed_row(buf, card, y, line);
        y += 1;
    }
    border_row(buf, card, y, '╰', '─', '╯');
    y += 1;

    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            // Half-line ends start and stop where the boxes' rounded corners do.
            border_row(buf, card, y, '╶', '─', '╴');
            y += 1;
        }
        let x = card.x + 2;
        match row {
            Row::Pair(label, value) => {
                put(buf, card, x, y, label, LABEL_WIDTH.min(inner));
                put(
                    buf,
                    card,
                    x + LABEL_WIDTH,
                    y,
                    value,
                    inner.saturating_sub(LABEL_WIDTH),
                );
            }
            Row::Full(line) => put(buf, card, x, y, line, inner),
        }
        y += 1;
    }

    border_row(buf, card, y, '╭', '─', '╮');
    boxed_row(buf, card, y + 1, footer);
    border_row(buf, card, y + 2, '╰', '─', '╯');
}

fn border_color(card: Rect, x: u16) -> Style {
    let t = f64::from(x - card.x) / f64::from(card.width.max(1));
    Style::new().fg(logo::gradient(t))
}

fn border_row(buf: &mut Buffer, card: Rect, y: u16, left: char, fill: char, right: char) {
    if y >= card.bottom() {
        return;
    }
    for x in card.left()..card.right() {
        let ch = if x == card.left() {
            left
        } else if x == card.right() - 1 {
            right
        } else {
            fill
        };
        buf[(x, y)].set_char(ch).set_style(border_color(card, x));
    }
}

fn boxed_row(buf: &mut Buffer, card: Rect, y: u16, line: &Line) {
    if y >= card.bottom() {
        return;
    }
    let (left, right) = (card.left(), card.right() - 1);
    buf[(left, y)]
        .set_char('│')
        .set_style(border_color(card, left));
    buf[(right, y)]
        .set_char('│')
        .set_style(border_color(card, right));
    put(buf, card, left + 2, y, line, card.width.saturating_sub(4));
}

/// Writes a line clipped to `max_width` and to the area, so a small terminal crops the
/// dashboard instead of crashing.
fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, line: &Line, max_width: u16) {
    if y >= area.bottom() || x >= area.right() {
        return;
    }
    let width = max_width.min(area.right() - x);
    buf.set_line(x, y, line, width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BeltMode;
    use crate::pad::demo::sample_history;
    use insta::assert_snapshot;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    static UTC: TimeZone = TimeZone::UTC;

    fn now() -> Timestamp {
        "2026-10-23T22:00:00Z".parse().unwrap()
    }

    fn walking() -> WalkingSession {
        WalkingSession {
            state: BeltState::Running,
            raw_state: 1,
            mode: BeltMode::Manual,
            speed: 3.5,
            distance_km: 1.38,
            steps: 2127,
            elapsed_seconds: 27 * 60 + 45,
        }
    }

    fn strava() -> StravaUiState {
        StravaUiState {
            connected: true,
            athlete_name: Some("Demo Walker".into()),
            muted: true,
            threshold_km: 0.7,
            pending_count: 27,
            below_threshold_count: 2,
        }
    }

    fn draw(view: &View, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, view)).unwrap();
        terminal
    }

    fn view<'a>(
        tab: Tab,
        session: &'a WalkingSession,
        connection: &'a ConnectionState,
        strava: &'a StravaUiState,
        history: &'a [SessionRecord],
    ) -> View<'a> {
        View {
            tab,
            session,
            connection,
            strava,
            history,
            now: now(),
            tz: &UTC,
        }
    }

    #[test]
    fn session_tab() {
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        let terminal = draw(&view(Tab::Session, &s, &c, &st, &[]), 90, 32);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn session_tab_hides_belt_and_speed_when_disconnected() {
        let (s, c, st) = (walking(), ConnectionState::Disconnected, strava());
        let terminal = draw(&view(Tab::Session, &s, &c, &st, &[]), 90, 32);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn statistics_tab() {
        let history = sample_history(now());
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        let terminal = draw(&view(Tab::Statistics, &s, &c, &st, &history), 90, 42);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn statistics_tab_without_history() {
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        let terminal = draw(&view(Tab::Statistics, &s, &c, &st, &[]), 90, 20);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn strava_tab() {
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        let terminal = draw(&view(Tab::Strava, &s, &c, &st, &[]), 90, 26);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn strava_tab_when_not_connected() {
        let (s, c, st) = (
            walking(),
            ConnectionState::Connected,
            StravaUiState::default(),
        );
        let terminal = draw(&view(Tab::Strava, &s, &c, &st, &[]), 90, 26);
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn every_tab_has_the_same_width() {
        let history = sample_history(now());
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        for tab in Tab::ALL {
            let terminal = draw(&view(tab, &s, &c, &st, &history), 120, 45);
            let buf = terminal.backend().buffer();
            let top = (0..120)
                .map(|x| buf[(x, CARD_TOP)].symbol().to_owned())
                .collect::<String>();
            assert_eq!(
                top.trim_end().chars().count(),
                CARD_WIDTH as usize,
                "{tab:?}"
            );
        }
    }

    #[test]
    fn warns_before_the_auto_rest_and_shows_errors() {
        let s = WalkingSession {
            elapsed_seconds: 2 * 3600 - 90,
            ..walking()
        };
        let c = ConnectionState::Error("No WalkingPad found after 30s.".into());
        let st = strava();
        let terminal = draw(&view(Tab::Session, &s, &c, &st, &[]), 90, 32);
        let text = format!("{:?}", terminal.backend());
        assert!(text.contains("1:58:30  Pad auto-rests in 01:30"), "{text}");
        assert!(
            text.contains("Error  No WalkingPad found after 30s."),
            "{text}"
        );
    }

    #[test]
    fn speed_bar_colour_follows_the_speed() {
        let colour_at = |speed: f64| {
            let s = WalkingSession { speed, ..walking() };
            let (c, st) = (ConnectionState::Connected, strava());
            let terminal = draw(&view(Tab::Session, &s, &c, &st, &[]), 90, 32);
            let buf = terminal.backend().buffer().clone();
            // The bar starts after "x.y km/h " in the Speed row.
            let speed_row = CARD_TOP + 4 + 4;
            let bar_x = 2 + LABEL_WIDTH + 9;
            (
                buf[(bar_x, speed_row)].symbol().to_owned(),
                buf[(bar_x, speed_row)].fg,
            )
        };
        assert_eq!(colour_at(1.5), ("█".into(), Color::Green));
        assert_eq!(colour_at(3.0), ("█".into(), Color::Yellow));
        assert_eq!(colour_at(5.0), ("█".into(), Color::Red));
        assert_eq!(colour_at(0.0).0, "░");
    }

    #[test]
    fn a_small_terminal_crops_instead_of_crashing() {
        let history = sample_history(now());
        let (s, c, st) = (walking(), ConnectionState::Connected, strava());
        for tab in Tab::ALL {
            draw(&view(tab, &s, &c, &st, &history), 30, 10);
            draw(&view(tab, &s, &c, &st, &history), 3, 3);
        }
    }

    #[test]
    fn tabs_cycle_both_ways() {
        assert_eq!(Tab::Session.next(), Tab::Statistics);
        assert_eq!(Tab::Strava.next(), Tab::Session);
        assert_eq!(Tab::Session.previous(), Tab::Strava);
    }
}
