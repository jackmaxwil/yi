//! Keys as xterm sends them, after Yi's console encoder.

use rune::{Key, KeyEvent};

fn control(c: char) -> Option<char> {
    let byte = match c.to_ascii_lowercase() {
        letter @ 'a'..='z' => u8::try_from(letter).ok()? & 0x1f,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(char::from(byte))
}

/// The bytes for `ev`, or None for a key the terminal leaves to the app (⌘ chords, modifier
/// presses). `base` is [`rune::Tx::key_base`]: Option as Meta sends ESC and the unmodified key.
pub fn encode(ev: &KeyEvent, base: Option<char>, app_cursor: bool) -> Option<String> {
    let m = ev.mods;
    if m.cmd && !m.ctrl {
        return None;
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let cursor = |end: char| match (modifier, app_cursor) {
        (1, true) => format!("\x1bO{end}"),
        (1, false) => format!("\x1b[{end}"),
        _ => format!("\x1b[1;{modifier}{end}"),
    };
    let tilde = |code: u8| match modifier {
        1 => format!("\x1b[{code}~"),
        _ => format!("\x1b[{code};{modifier}~"),
    };
    let sequence = match &ev.key {
        Key::Up => Some(cursor('A')),
        Key::Down => Some(cursor('B')),
        Key::Right => Some(cursor('C')),
        Key::Left => Some(cursor('D')),
        Key::Home => Some(cursor('H')),
        Key::End => Some(cursor('F')),
        Key::Insert => Some(tilde(2)),
        Key::Delete => Some(tilde(3)),
        Key::PageUp => Some(tilde(5)),
        Key::PageDown => Some(tilde(6)),
        Key::Function(n @ 1..=4) => Some(format!("\x1bO{}", char::from(b'O'.saturating_add(*n)))),
        Key::Function(n) => [15_u8, 17, 18, 19, 20, 21, 23, 24]
            .get(usize::from(*n).saturating_sub(5))
            .map(|code| tilde(*code)),
        _ => None,
    };
    if sequence.is_some() {
        return sequence;
    }
    let body = match &ev.key {
        Key::Text(t) if m.ctrl => {
            let c = base.or_else(|| t.chars().next())?;
            control(c).map_or_else(|| t.clone(), String::from)
        }
        Key::Text(t) if m.alt => base.map_or_else(|| t.clone(), String::from),
        Key::Text(t) => t.clone(),
        Key::Enter => "\r".to_owned(),
        Key::Backspace if m.ctrl => "\x08".to_owned(),
        Key::Backspace => "\x7f".to_owned(),
        Key::Tab if m.shift => "\x1b[Z".to_owned(),
        Key::Tab => "\t".to_owned(),
        Key::Escape => "\x1b".to_owned(),
        _ => return None,
    };
    Some(if m.alt { format!("\x1b{body}") } else { body })
}

/// Incident (Yi's console): a clipboard holding `ESC[201~` closed the bracket early and the
/// rest reached the shell as typed keys; ESC never belongs in pasted text.
pub fn paste(text: &str, bracketed: bool) -> String {
    let text = text.replace('\x1b', "");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, paste};
    use rune::{Key, KeyEvent, Mods};
    use std::error::Error;

    type Case = (Key, Mods, Option<char>, bool, &'static str);

    fn ev(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    /// Dies with a shell that cannot be driven: an arrow that moves nothing in readline, a
    /// Ctrl-C that never interrupts, an Option chord that types `å` instead of Meta-a.
    #[test]
    fn keys_encode_as_xterm_sends_them() -> Result<(), Box<dyn Error>> {
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..none };
        let alt = Mods { alt: true, ..none };
        let shift = Mods {
            shift: true,
            ..none
        };
        let text = |s: &str| Key::Text(s.into());
        let cases: [Case; 16] = [
            (Key::Up, none, None, false, "\x1b[A"),
            (Key::Up, none, None, true, "\x1bOA"),
            (Key::Left, ctrl, None, true, "\x1b[1;5D"),
            (text("c"), ctrl, Some('c'), false, "\x03"),
            (text("d"), ctrl, None, false, "\x04"),
            (text("å"), alt, Some('a'), false, "\x1ba"),
            (text("é"), none, None, false, "é"),
            (text("日"), none, None, false, "日"),
            (Key::Backspace, none, None, false, "\x7f"),
            (Key::Backspace, alt, None, false, "\x1b\x7f"),
            (Key::Enter, none, None, false, "\r"),
            (Key::Tab, none, None, false, "\t"),
            (Key::Tab, shift, None, false, "\x1b[Z"),
            (Key::Delete, none, None, false, "\x1b[3~"),
            (Key::Function(1), none, None, false, "\x1bOP"),
            (Key::Function(5), none, None, false, "\x1b[15~"),
        ];
        for (key, mods, base, app, want) in cases {
            let got = encode(&ev(key.clone(), mods), base, app).ok_or("key not encoded")?;
            assert_eq!(got, want, "{key:?} {mods:?}");
        }
        let cmd = Mods {
            cmd: true,
            logo: true,
            ..none
        };
        assert_eq!(
            encode(&ev(Key::Command('c'), cmd), None, false),
            None,
            "⌘C is the app's copy"
        );
        Ok(())
    }

    #[test]
    fn a_paste_cannot_close_its_own_bracket() {
        assert_eq!(
            paste("a\x1b[201~; rm x\n", true),
            "\x1b[200~a[201~; rm x\n\x1b[201~"
        );
    }
}
