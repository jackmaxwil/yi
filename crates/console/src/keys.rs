//! Binding model: ⌘ chords where the kitty protocol reports them, alt-chords
//! everywhere, a ctrl+b prefix table for terminals that eat alt. One vocabulary.

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
    ToggleSidebar,
    ToggleNotebook,
    ToggleDiff,
}

/// The prefix key: ctrl+b, held as a one-shot armed state by the caller.
pub fn is_prefix(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Direct chords — alt everything, so plain typing always reaches the
/// composer. Digits map tabs.
pub fn direct(key: &KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return super_chord(key);
    }
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
        KeyCode::Char('j' | 'J') if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(Action::ToggleNotebook)
        }
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
        KeyCode::Char('b') => Some(Action::ToggleSidebar),
        KeyCode::Char('g') => Some(Action::ToggleDiff),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectTab(n))
        }
        _ => None,
    }
}

fn super_chord(key: &KeyEvent) -> Option<Action> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char('\\') | KeyCode::Char('|') if shift => Some(Action::SplitDown),
        KeyCode::Char('\\') => Some(Action::SplitRight),
        KeyCode::Char('d') | KeyCode::Char('D') if shift => Some(Action::SplitDown),
        KeyCode::Char('d') => Some(Action::SplitRight),
        KeyCode::Char('x') | KeyCode::Char('w') => Some(Action::ClosePane),
        KeyCode::Char('m') | KeyCode::Char('M') if shift => Some(Action::Zoom),
        KeyCode::Char('t') | KeyCode::Char('T') if shift => Some(Action::NewTab),
        KeyCode::Char('n') | KeyCode::Char('N') if shift => Some(Action::NewSession),
        KeyCode::Char('p') => Some(Action::Navigator),
        KeyCode::Char('b') => Some(Action::ToggleSidebar),
        KeyCode::Char('j') => Some(Action::ToggleNotebook),
        KeyCode::Char('g') => Some(Action::ToggleDiff),
        KeyCode::Char('.') => Some(Action::CancelTurn),
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
pub fn hint(prefix_armed: bool, cmd: bool) -> &'static str {
    if prefix_armed {
        "PREFIX  v split│ s split─ x close z zoom h/j/k/l focus c tab 1..9 tab g nav q quit"
    } else if cmd {
        "⌘\\ split  ⌥←→↑↓ focus  ⌘⇧M zoom  ⌘X close  ⌘⇧T/⌘1..9 tabs  ⌘P nav  ⌘⇧N new  ⌘B side  ⌘J nb  ⌘G diff"
    } else {
        "⌥v/⌥s split  ⌥←→↑↓ focus  ⌥z zoom  ⌥x close  ⌥t/⌥1..9 tabs  ⌥/ nav  ⌥n new  ⌥b side  ctrl+b prefix"
    }
}
