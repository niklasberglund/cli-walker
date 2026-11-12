//! Terminal UI. For now this is a placeholder screen that exercises the same terminal setup the
//! dashboard will use (raw mode, inline viewport, keyboard input), so the test harness for it
//! exists before the dashboard is ported.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, TerminalOptions, Viewport,
    layout::Alignment,
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, BorderType, Paragraph},
};

/// Height of the inline viewport, in terminal rows.
pub const VIEWPORT_HEIGHT: u16 = 5;

/// Draws the placeholder screen.
pub fn render(frame: &mut Frame) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(" CLI-Walker ").bold())
        .border_style(Style::new().cyan());
    let text = vec![
        Line::from("The Rust port is not functional yet."),
        Line::from(vec!["Press ".into(), "q".bold(), " to quit.".into()]),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(block),
        frame.area(),
    );
}

/// Whether a key press should end the program: `q`, Esc or Ctrl+C.
pub fn is_quit(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && match key.code {
            KeyCode::Char('q') | KeyCode::Esc => true,
            KeyCode::Char('c') => key.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        }
}

/// Shows the placeholder screen in an inline viewport until the user quits.
pub fn run() -> io::Result<()> {
    let mut terminal = ratatui::try_init_with_options(TerminalOptions {
        viewport: Viewport::Inline(VIEWPORT_HEIGHT),
    })?;
    let result = (|| loop {
        terminal.draw(render)?;
        if let Event::Key(key) = event::read()?
            && is_quit(&key)
        {
            return Ok(());
        }
    })();
    drop(terminal);
    ratatui::try_restore()?;
    println!();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn placeholder_screen() {
        let mut terminal = Terminal::new(TestBackend::new(50, VIEWPORT_HEIGHT)).unwrap();
        terminal.draw(render).unwrap();
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn quit_keys() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        assert!(is_quit(&key(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(is_quit(&key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(is_quit(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!is_quit(&key(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert!(!is_quit(&key(KeyCode::Char('x'), KeyModifiers::NONE)));
    }
}
