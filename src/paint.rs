//! Styling for plain command output. Everything is dropped when stdout is not a terminal or
//! `NO_COLOR` is set, so piped output carries no escape codes.

use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::style::{Attribute, Color, Stylize};

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub trait Paint: AsRef<str> {
    fn paint(&self, color: Option<Color>, attribute: Option<Attribute>) -> String {
        let text = self.as_ref();
        if !ENABLED.load(Ordering::Relaxed) {
            return text.to_owned();
        }
        let mut styled = text.stylize();
        if let Some(color) = color {
            styled = styled.with(color);
        }
        if let Some(attribute) = attribute {
            styled = styled.attribute(attribute);
        }
        styled.to_string()
    }

    fn red(&self) -> String {
        self.paint(Some(Color::Red), None)
    }
    fn green(&self) -> String {
        self.paint(Some(Color::Green), None)
    }
    fn yellow(&self) -> String {
        self.paint(Some(Color::Yellow), None)
    }
    fn bold(&self) -> String {
        self.paint(None, Some(Attribute::Bold))
    }
    fn dim(&self) -> String {
        self.paint(None, Some(Attribute::Dim))
    }
}

impl<T: AsRef<str> + ?Sized> Paint for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_only_when_enabled() {
        set_enabled(true);
        assert!(Paint::green("ok").starts_with('\u{1b}'));
        assert!(Paint::bold("ok").contains("\u{1b}[1m"));
        set_enabled(false);
        assert_eq!(Paint::green("ok"), "ok");
        assert_eq!(Paint::bold("ok"), "ok");
        set_enabled(true);
    }
}
