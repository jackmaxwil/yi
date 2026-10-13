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
    ToggleTape,
    OpenEditor,
    Find,
    Save,
    Undo,
    Redo,
    Menu,
    Settings,
    OpenBrowser,
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
        KeyCode::Char('y') => Some(Action::ToggleTape),
        KeyCode::Char('e') => Some(Action::OpenEditor),
        KeyCode::Char('?') => Some(Action::Keys),
        KeyCode::Char('c') => Some(Action::Menu),
        KeyCode::Char(',') => Some(Action::Settings),
        KeyCode::Char('w') => Some(Action::OpenBrowser),
        KeyCode::Char(digit @ '1'..='9') => {
            let n = u8::try_from(u32::from(digit).saturating_sub(u32::from('0'))).ok()?;
            Some(Action::SelectSlot(n))
        }
        _ => None,
    }
}

fn super_chord(key: &KeyEvent) -> Option<Action> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let shift = key.modifiers.contains(KeyModifiers::SHIFT) || c.is_ascii_uppercase();
    let action = match (c.to_ascii_lowercase(), shift) {
        ('|', _) | ('\\', true) => Action::SplitDown,
        ('\\', false) => Action::SplitRight,
        ('x', false) => Action::ClosePane,
        ('m', true) => Action::Zoom,
        ('o', true) => Action::NewTab,
        ('n', true) => Action::NewSession,
        ('r', true) => Action::ToggleDiff,
        ('l', true) => Action::Find,
        ('u', true) => Action::Redo,
        ('p', false) => Action::Navigator,
        ('b', false) => Action::ToggleSidebar,
        ('b', true) => Action::OpenBrowser,
        ('i', false) => Action::ToggleNotebook,
        ('y', false) => Action::ToggleTape,
        ('o', false) => Action::OpenEditor,
        ('s', false) => Action::Save,
        ('u', false) => Action::Undo,
        ('/', false) => Action::Keys,
        _ => return None,
    };
    Some(action)
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
        KeyCode::Char('m') => Some(Action::Menu),
        KeyCode::Char(',') => Some(Action::Settings),
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

impl Chord {
    pub fn key(&self, cmd: bool) -> &'static str {
        if cmd { self.cmd } else { self.alt }
    }
}

pub fn chord_for(action: Action, cmd: bool) -> &'static str {
    CHORDS
        .iter()
        .find(|chord| chord.action == Some(action))
        .map_or("", |chord| chord.key(cmd))
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

pub const CHORDS: [Chord; 25] = [
    chord("command palette", "⌥/", "⌘P", "g", Some(Action::Navigator)),
    chord("new session", "⌥n", "⌘⇧N", "o", Some(Action::NewSession)),
    chord("jump to rail slot 1..9", "⌥1..9", "⌥1..9", "", None),
    chord("split right", "⌥v", "⌘\\", "v", Some(Action::SplitRight)),
    chord("split down", "⌥s", "⌘⇧\\", "s", Some(Action::SplitDown)),
    chord("close pane", "⌥x", "⌘X", "x", Some(Action::ClosePane)),
    chord("zoom pane", "⌥z", "⌘⇧M", "z", Some(Action::Zoom)),
    chord(
        "menu for the pane or session",
        "⌥c",
        "⌥c",
        "m",
        Some(Action::Menu),
    ),
    chord("focus pane", "⌥←→↑↓", "⌥←→↑↓", "h j k l", None),
    chord(
        "notebook pane",
        "⌥⇧J",
        "⌘I",
        "",
        Some(Action::ToggleNotebook),
    ),
    chord("review pane", "⌥g", "⌘⇧R", "", Some(Action::ToggleDiff)),
    chord(
        "tape: where the time went",
        "⌥y",
        "⌘Y",
        "",
        Some(Action::ToggleTape),
    ),
    chord("browser pane", "⌥w", "⌘⇧B", "", Some(Action::OpenBrowser)),
    chord(
        "open a file in a pane",
        "⌥e",
        "⌘O",
        "",
        Some(Action::OpenEditor),
    ),
    chord(
        "editor: save, find, undo, redo",
        "",
        "⌘S ⌘⇧L ⌘U ⌘⇧U",
        "",
        None,
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
        "⌘⇧O",
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
    chord("settings", "⌥,", "⌥,", ",", Some(Action::Settings)),
    chord("all keys, in settings", "⌥?", "⌘/", "", Some(Action::Keys)),
    chord(
        "chat: cycle normal · thinking · verbose",
        "ctrl+o",
        "ctrl+o",
        "",
        None,
    ),
    chord("chat: hide or show the HUD", "ctrl+t", "ctrl+t", "", None),
    chord(
        "sidebar: ? needs you · ◐ working · ● done, unseen · ○ idle · ! stuck · ✕ failed",
        "",
        "",
        "",
        None,
    ),
    chord(
        "status: ≥$ is a lower bound; a reply came back without its usage",
        "",
        "",
        "",
        None,
    ),
];

/// One hint-row key and its drop rank: the row sheds the lowest rank first, whole,
/// and `KEEP` never. The prefix row sheds `g palette` first: `⌥/` already names it.
type Hint = (&'static str, u8);
const KEEP: u8 = u8::MAX;

const PREFIX_HINTS: [Hint; 10] = [
    ("PREFIX ", KEEP),
    ("v split│", 7),
    ("s split─", 6),
    ("x close", 5),
    ("z zoom", 4),
    ("h/j/k/l focus", 3),
    ("c tab", 2),
    ("1..9 tab", 1),
    ("g palette", 0),
    ("q leave", KEEP),
];
const HINTS: [(Action, &str, u8); 6] = [
    (Action::Navigator, "command palette", 4),
    (Action::NewSession, "new session", 3),
    (Action::ToggleSidebar, "sidebar", 1),
    (Action::ToggleNotebook, "notebook", 0),
    (Action::ToggleDiff, "diff", 2),
    (Action::Keys, "keys", KEEP),
];

/// The hint row, indented three cells, that fits `width` cells: keys drop whole, by
/// rank, never cut mid-key.
pub fn hint(prefix_armed: bool, cmd: bool, width: usize) -> String {
    let sep = hint_sep(prefix_armed);
    let kept: Vec<String> = hint_items(prefix_armed, cmd, width)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    format!("   {}", kept.join(sep))
}

pub fn hint_sep(prefix_armed: bool) -> &'static str {
    if prefix_armed { " " } else { "   " }
}

/// The keys [`hint`] keeps, each with the action a press on it runs; the prefix row has none.
pub fn hint_items(prefix_armed: bool, cmd: bool, width: usize) -> Vec<(String, Option<Action>)> {
    let items: Vec<(String, Option<Action>, u8)> = if prefix_armed {
        PREFIX_HINTS
            .iter()
            .map(|(text, rank)| ((*text).to_owned(), None, *rank))
            .collect()
    } else {
        HINTS
            .iter()
            .map(|(action, what, rank)| {
                (
                    format!("{} {what}", chord_for(*action, cmd)),
                    Some(*action),
                    *rank,
                )
            })
            .collect()
    };
    let mut floor = 0;
    loop {
        let kept: Vec<(String, Option<Action>)> = items
            .iter()
            .filter(|(_, _, rank)| *rank >= floor)
            .map(|(text, action, _)| (text.clone(), *action))
            .collect();
        let texts: Vec<&str> = kept.iter().map(|(text, _)| text.as_str()).collect();
        let row = format!("   {}", texts.join(hint_sep(prefix_armed)));
        if floor == KEEP || ratatui::text::Line::raw(row.as_str()).width() <= width {
            return kept;
        }
        floor += 1;
    }
}

#[cfg(test)]
mod hint_tests {
    use super::hint;

    #[test]
    fn the_row_drops_whole_keys_by_rank_to_fit() {
        let full = "   ⌥/ command palette   ⌥n new session   ⌥b sidebar   ⌥⇧J notebook   ⌥g diff   ⌥? keys";
        let no_notebook = "   ⌥/ command palette   ⌥n new session   ⌥b sidebar   ⌥g diff   ⌥? keys";
        let cases = [
            (false, false, 86, full),
            (false, false, 85, no_notebook),
            (false, false, 71, no_notebook),
            (
                false,
                false,
                70,
                "   ⌥/ command palette   ⌥n new session   ⌥g diff   ⌥? keys",
            ),
            (false, false, 0, "   ⌥? keys"),
            (
                false,
                true,
                63,
                "   ⌘P command palette   ⌘⇧N new session   ⌘⇧R diff   ⌘/ keys",
            ),
            (true, false, 18, "   PREFIX  q leave"),
        ];
        for (armed, cmd, width, want) in cases {
            assert_eq!(
                hint(armed, cmd, width),
                want,
                "armed {armed} cmd {cmd} width {width}"
            );
        }
    }
}
