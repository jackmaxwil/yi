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
