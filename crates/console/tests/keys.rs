use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use yi_console::keys::{CHORDS, direct};

/// Incident: Ghostty 1.3.1 and kitty 0.49.2 take these ⌘ keys first, by default keybind or
/// menu (`ghostty +list-keybinds --default`, kitty.conf), so the console never sees them.
const HELD: &str = "a c d e f g h j k l m n q r t v w z 0 1 2 3 4 5 6 7 8 9 , = + - [ ] ` \
                    ⇧d ⇧f ⇧g ⇧i ⇧j ⇧p ⇧t ⇧v ⇧w ⇧z ⇧/ ⇧? ⇧[ ⇧] ⇧, ⇧= ⇧- ⇧`";

fn held() -> impl Iterator<Item = (char, bool)> {
    HELD.split_whitespace()
        .filter_map(|token| match token.strip_prefix('⇧') {
            Some(rest) => rest.chars().next().map(|c| (c, true)),
            None => token.chars().next().map(|c| (c, false)),
        })
}

#[test]
fn no_cmd_chord_is_one_the_terminal_takes() {
    for (c, shift) in held() {
        let (modifiers, codes) = if shift {
            (
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
                vec![c, c.to_ascii_uppercase()],
            )
        } else {
            (KeyModifiers::SUPER, vec![c])
        };
        for code in codes {
            let key = KeyEvent::new(KeyCode::Char(code), modifiers);
            assert_eq!(
                direct(&key),
                None,
                "⌘{}{code}",
                if shift { "⇧" } else { "" }
            );
        }
    }
}

#[test]
fn no_hint_names_a_key_the_terminal_takes() {
    let named: Vec<&str> = CHORDS
        .iter()
        .flat_map(|chord| chord.cmd.split_whitespace())
        .collect();
    for (c, shift) in held() {
        let label = match (shift, c.is_ascii_digit()) {
            (_, true) => "⌘1..9".to_owned(),
            (true, false) => format!("⌘⇧{}", c.to_ascii_uppercase()),
            (false, false) => format!("⌘{}", c.to_ascii_uppercase()),
        };
        assert!(!named.contains(&label.as_str()), "{label} is advertised");
    }
}

#[test]
fn the_header_buttons_and_terminal_keep_chords_the_terminal_leaves_alone() {
    use yi_console::keys::Action;
    let shifted = KeyModifiers::SUPER | KeyModifiers::SHIFT;
    let cases = [
        (KeyModifiers::ALT, 'y', Action::OpenTerminal),
        (KeyModifiers::SUPER, 'y', Action::OpenTerminal),
        (shifted, 'B', Action::OpenBrowser),
        (KeyModifiers::ALT, 'w', Action::OpenBrowser),
        (KeyModifiers::SUPER, 'o', Action::OpenEditor),
        (shifted, 'R', Action::ToggleDiff),
    ];
    for (modifiers, c, want) in cases {
        let key = KeyEvent::new(KeyCode::Char(c), modifiers);
        assert_eq!(direct(&key), Some(want), "{modifiers:?} {c}");
    }
}

#[test]
fn terminal_environments_share_graphics_support_but_not_chord_support() {
    let environments = [
        (
            "Ghostty TERM",
            "xterm-ghostty",
            None,
            None,
            None,
            true,
            true,
        ),
        (
            "Ghostty TERM_PROGRAM",
            "xterm-256color",
            Some("ghostty"),
            None,
            None,
            true,
            true,
        ),
        ("kitty", "xterm-kitty", None, Some("1"), None, true, true),
        (
            "kitty by window id",
            "xterm-256color",
            None,
            Some("1"),
            None,
            true,
            false,
        ),
        (
            "tmux in kitty",
            "tmux-256color",
            Some("tmux"),
            Some("1"),
            Some("TMUX"),
            false,
            false,
        ),
        (
            "xterm-kitty in tmux",
            "xterm-kitty",
            None,
            Some("1"),
            Some("TMUX"),
            false,
            true,
        ),
        (
            "xterm-kitty in screen",
            "xterm-kitty",
            None,
            Some("1"),
            Some("STY"),
            false,
            true,
        ),
        ("xterm", "xterm-256color", None, None, None, false, false),
    ];
    for (name, term, program, window_id, multiplexer, graphics, chords) in environments {
        let var = |key: &str| -> Option<std::ffi::OsString> {
            let value = match key {
                "TERM" => Some(term),
                "TERM_PROGRAM" => program,
                "KITTY_WINDOW_ID" => window_id,
                _ => (multiplexer == Some(key)).then_some("1"),
            };
            value.map(std::ffi::OsString::from)
        };
        assert_eq!(
            yi_tui::orb::kitty::supported_with(var),
            graphics,
            "{name} graphics"
        );
        assert_eq!(
            yi_tui::orb::kitty::kitty_named_terminal(Some(term), program),
            chords,
            "{name} chords"
        );
    }
}
