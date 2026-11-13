//! Maps key presses to dashboard actions.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Up or down; `fine` with Ctrl held.
    Up {
        fine: bool,
    },
    Down {
        fine: bool,
    },
    NextTab,
    PreviousTab,
    ToggleBelt,
    ToggleMute,
    Quit,
}

pub fn action(key: &KeyEvent) -> Option<Action> {
    // Windows reports releases too; only presses act.
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        // Raw mode delivers Ctrl+C as a key, not a signal, so quitting goes through the
        // normal path: the belt is stopped and the session saved.
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('q' | 'Q') => Action::Quit,
        KeyCode::Char(' ') => Action::ToggleBelt,
        KeyCode::Char('m' | 'M') => Action::ToggleMute,
        KeyCode::Tab | KeyCode::Right => Action::NextTab,
        KeyCode::BackTab | KeyCode::Left => Action::PreviousTab,
        KeyCode::Up => Action::Up { fine: ctrl },
        KeyCode::Down => Action::Down { fine: ctrl },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        action(&KeyEvent::new(code, modifiers))
    }

    #[test]
    fn maps_the_documented_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(press(KeyCode::Char('q'), none), Some(Action::Quit));
        assert_eq!(press(KeyCode::Char('Q'), none), Some(Action::Quit));
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c'), none), None);
        assert_eq!(press(KeyCode::Char(' '), none), Some(Action::ToggleBelt));
        assert_eq!(press(KeyCode::Char('m'), none), Some(Action::ToggleMute));
        assert_eq!(press(KeyCode::Tab, none), Some(Action::NextTab));
        assert_eq!(press(KeyCode::Right, none), Some(Action::NextTab));
        assert_eq!(press(KeyCode::Left, none), Some(Action::PreviousTab));
        assert_eq!(press(KeyCode::Up, none), Some(Action::Up { fine: false }));
        assert_eq!(
            press(KeyCode::Up, KeyModifiers::CONTROL),
            Some(Action::Up { fine: true })
        );
        assert_eq!(
            press(KeyCode::Down, KeyModifiers::CONTROL),
            Some(Action::Down { fine: true })
        );
        assert_eq!(press(KeyCode::Char('x'), none), None);
        assert_eq!(press(KeyCode::Esc, none), None);
    }

    #[test]
    fn ignores_key_releases() {
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action(&key), None);
    }
}
