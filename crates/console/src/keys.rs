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
    /// Resume the nth rail row into the focused pane.
    SelectSlot(u8),
    Quit,
    StopDaemon,
    Keys,
    ToggleSidebar,
    ToggleNotebook,
    ToggleDiff,
    OpenEditor,
    Find,
    Save,
    Undo,
    Redo,
}

/// The prefix key: ctrl+b, held as a one-shot armed state by the caller.
pub fn is_prefix(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Direct chords — alt everything, so plain typing always reaches the
/// composer. Digits jump to rail slots; tabs are `ctrl+b 1..9`.
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
        KeyCode::Char('e') => Some(Action::OpenEditor),
        KeyCode::Char('?') => Some(Action::Keys),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectSlot(n))
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
        KeyCode::Char('e') => Some(Action::OpenEditor),
        KeyCode::Char('f') => Some(Action::Find),
        KeyCode::Char('s') => Some(Action::Save),
        KeyCode::Char('z' | 'Z') if shift => Some(Action::Redo),
        KeyCode::Char('z') => Some(Action::Undo),
        KeyCode::Char('?') | KeyCode::Char('/') => Some(Action::Keys),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectSlot(n))
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

pub struct Chord {
    pub what: &'static str,
    pub alt: &'static str,
    pub cmd: &'static str,
    pub prefix: &'static str,
    pub action: Option<Action>,
}

const fn chord(
    what: &'static str,
    alt: &'static str,
    cmd: &'static str,
    prefix: &'static str,
    action: Option<Action>,
) -> Chord {
    Chord {
        what,
        alt,
        cmd,
        prefix,
        action,
    }
}

pub const CHORDS: [Chord; 16] = [
    chord("command palette", "⌥/", "⌘P", "g", Some(Action::Navigator)),
    chord("new session", "⌥n", "⌘⇧N", "o", Some(Action::NewSession)),
    chord("jump to rail slot 1..9", "⌥1..9", "⌘1..9", "", None),
    chord("split right", "⌥v", "⌘\\", "v", Some(Action::SplitRight)),
    chord("split down", "⌥s", "⌘⇧\\", "s", Some(Action::SplitDown)),
    chord("close pane", "⌥x", "⌘X", "x", Some(Action::ClosePane)),
    chord("zoom pane", "⌥z", "⌘⇧M", "z", Some(Action::Zoom)),
    chord("focus pane", "⌥←→↑↓", "⌥←→↑↓", "h j k l", None),
    chord(
        "notebook pane",
        "⌥⇧J",
        "⌘J",
        "",
        Some(Action::ToggleNotebook),
    ),
    chord("diff pane", "⌥g", "⌘G", "", Some(Action::ToggleDiff)),
    chord(
        "open a file in a pane",
        "⌥e",
        "⌘E",
        "",
        Some(Action::OpenEditor),
    ),
    chord(
        "sidebar: rail or full",
        "⌥b",
        "⌘B",
        "",
        Some(Action::ToggleSidebar),
    ),
    chord(
        "new tab · next · previous",
        "⌥t ⌥] ⌥[",
        "⌘⇧T",
        "c n p 1..9",
        Some(Action::NewTab),
    ),
    chord(
        "leave; the daemon keeps running",
        "⌥q",
        "⌥q",
        "q",
        Some(Action::Quit),
    ),
    chord(
        "stop the daemon and quit",
        "ctrl+c ctrl+c",
        "ctrl+c ctrl+c",
        "",
        Some(Action::StopDaemon),
    ),
    chord("all keys", "⌥?", "⌘?", "", Some(Action::Keys)),
];

pub fn hint(prefix_armed: bool, cmd: bool) -> &'static str {
    if prefix_armed {
        "PREFIX  v split│ s split─ x close z zoom h/j/k/l focus c tab 1..9 tab g palette q leave"
    } else if cmd {
        "⌘P command palette   ⌘⇧N new session   ⌘B sidebar   ⌘J notebook   ⌘G diff   ⌘? keys"
    } else {
        "⌥/ command palette   ⌥n new session   ⌥b sidebar   ⌥⇧J notebook   ⌥g diff   ⌥? keys"
    }
}
