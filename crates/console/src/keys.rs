//! Binding model: direct alt-chords by default, a ctrl+b prefix table as
//! the fallback for terminals that eat alt. One action vocabulary for both.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SplitRight,
    SplitDown,
    ClosePane,
    Zoom,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    NewTab,
    NextTab,
    PrevTab,
    SelectTab(u8),
    NewSession,
    Navigator,
    ToggleZone,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    CancelTurn,
    Quit,
}

/// The prefix key: ctrl+b, held as a one-shot armed state by the caller.
pub fn is_prefix(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Direct chords — alt everything, so plain typing always reaches the
/// composer. Digits map tabs.
pub fn direct(key: &KeyEvent) -> Option<Action> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if !alt {
        return match key.code {
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(Action::ToggleZone)
            }
            KeyCode::PageUp => Some(Action::PageUp),
            KeyCode::PageDown => Some(Action::PageDown),
            KeyCode::Esc => Some(Action::CancelTurn),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('v') => Some(Action::SplitRight),
        KeyCode::Char('s') => Some(Action::SplitDown),
        KeyCode::Char('x') => Some(Action::ClosePane),
        KeyCode::Char('z') => Some(Action::Zoom),
        KeyCode::Left => Some(Action::FocusLeft),
        KeyCode::Right => Some(Action::FocusRight),
        KeyCode::Up => Some(Action::FocusUp),
        KeyCode::Down => Some(Action::FocusDown),
        KeyCode::Char('t') => Some(Action::NewTab),
        KeyCode::Char(']') => Some(Action::NextTab),
        KeyCode::Char('[') => Some(Action::PrevTab),
        KeyCode::Char('n') => Some(Action::NewSession),
        KeyCode::Char('/') => Some(Action::Navigator),
        KeyCode::Char('k') => Some(Action::ScrollUp),
        KeyCode::Char('j') => Some(Action::ScrollDown),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectTab(n))
        }
        _ => None,
    }
}

/// The one-shot table after ctrl+b. Unbound keys simply disarm.
pub fn prefixed(key: &KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('v') => Some(Action::SplitRight),
        KeyCode::Char('s') | KeyCode::Char('-') => Some(Action::SplitDown),
        KeyCode::Char('x') => Some(Action::ClosePane),
        KeyCode::Char('z') => Some(Action::Zoom),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::FocusLeft),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::FocusRight),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::FocusUp),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::FocusDown),
        KeyCode::Char('c') => Some(Action::NewTab),
        KeyCode::Char('n') => Some(Action::NextTab),
        KeyCode::Char('p') => Some(Action::PrevTab),
        KeyCode::Char('o') => Some(Action::NewSession),
        KeyCode::Char('g') => Some(Action::Navigator),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectTab(n))
        }
        _ => None,
    }
}

/// The mode-bar hint for the current input state.
pub fn hint(prefix_armed: bool) -> &'static str {
    if prefix_armed {
        "PREFIX  v split│ s split─ x close z zoom h/j/k/l focus c tab 1..9 tab g nav q quit"
    } else {
        "⌥v/⌥s split  ⌥←→↑↓ focus  ⌥z zoom  ⌥x close  ⌥t/⌥1..9 tabs  ⌥/ nav  ⌥n new  ctrl+b prefix"
    }
}
