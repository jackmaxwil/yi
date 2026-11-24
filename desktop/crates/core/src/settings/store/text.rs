//! The settings as text: a TOML-like file of the settings that differ from their defaults,
//! and the reading of it back, line by line.

use std::ops::Range;

use serde_json::Value;

use crate::ids::SettingKey;
use crate::settings::schema::{Control, Setting, SettingValue};
use crate::settings::store::value::{read as read_value, written};

/// What one line of the text says.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Blank,
    Comment,
    Section,
    /// A setting and the value written for it, which its control accepts.
    Set(SettingKey, SettingValue),
    /// A line that does not parse, or names no setting, or holds a value the control refuses.
    Bad(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// The line's bytes in the text, without its newline.
    pub bytes: Range<usize>,
    /// Where the value starts, on a line that has one.
    pub value_at: Option<usize>,
    pub mark: Mark,
}

fn editable(row: &&Setting) -> bool {
    row.changed() && !matches!(row.control, Control::Other(_))
}

/// The settings that differ from their defaults, in sections named by the first part of
/// each key; a key without a dot comes first, under no heading.
pub fn render(rows: &[Setting]) -> String {
    let shown: Vec<&Setting> = rows.iter().filter(editable).collect();
    let mut sections: Vec<(&str, Vec<(&str, String)>)> = Vec::new();
    for row in &shown {
        let (section, name) = row
            .key
            .as_str()
            .split_once('.')
            .unwrap_or(("", row.key.as_str()));
        let value = row.value.as_ref().map(written).unwrap_or_default();
        match sections.iter_mut().find(|(each, _)| *each == section) {
            Some((_, pairs)) => pairs.push((name, value)),
            None => sections.push((section, vec![(name, value)])),
        }
    }
    sections.sort_by_key(|(section, _)| !section.is_empty());
    let count = shown.len();
    let mut out = match count {
        1 => "# 1 setting differs from its default\n".to_owned(),
        _ => format!("# {count} settings differ from the defaults\n"),
    };
    for (section, pairs) in sections {
        out.push('\n');
        if !section.is_empty() {
            out.push_str(&format!("[{section}]\n"));
        }
        let width = pairs
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        for (name, value) in pairs {
            out.push_str(&format!("{name:<width$} = {value}\n"));
        }
    }
    out
}

fn comment_or_blank(tail: &str) -> bool {
    let tail = tail.trim();
    tail.is_empty() || tail.starts_with('#')
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "_.-".contains(c))
}

/// The value at the start of `rest`, with only a comment after it.
fn value_of(rest: &str) -> Result<Value, String> {
    let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    let first = stream.next();
    let after = rest.get(stream.byte_offset()..).unwrap_or_default();
    match first {
        Some(Ok(value)) if comment_or_blank(after) => Ok(value),
        _ => Err("a value is text in quotes, a number, true or false".to_owned()),
    }
}

fn pair(line: &str, section: &str, rows: &[Setting]) -> Mark {
    let Some((name, rest)) = line.split_once('=') else {
        return Mark::Bad("a setting is written name = value".to_owned());
    };
    let name = name.trim();
    if !plain_name(name) {
        return Mark::Bad(format!("{name:?} is not a setting name"));
    }
    let key = match section {
        "" => name.to_owned(),
        _ => format!("{section}.{name}"),
    };
    let Some(row) = rows.iter().find(|row| row.key.as_str() == key) else {
        return Mark::Bad(format!("no setting named {key}"));
    };
    match value_of(rest).and_then(|json| read_value(row, &json).map_err(|why| why.to_string())) {
        Ok(value) => Mark::Set(row.key.clone(), value),
        Err(why) => Mark::Bad(why),
    }
}

fn one(line: &str, section: &mut String, rows: &[Setting]) -> Mark {
    let body = line.trim();
    if body.is_empty() {
        return Mark::Blank;
    }
    if body.starts_with('#') {
        return Mark::Comment;
    }
    if let Some(inner) = body.strip_prefix('[') {
        return match inner.split_once(']') {
            Some((name, tail)) if plain_name(name.trim()) && comment_or_blank(tail) => {
                *section = name.trim().to_owned();
                Mark::Section
            }
            _ => Mark::Bad("a section is written [name]".to_owned()),
        };
    }
    pair(body, section, rows)
}

/// Every line of `text`, marked. `rows` are the settings a line may name.
pub fn read(text: &str, rows: &[Setting]) -> Vec<Seen> {
    let mut section = String::new();
    let mut at = 0usize;
    let mut seen = Vec::new();
    for line in text.split('\n') {
        let bytes = at..at.saturating_add(line.len());
        at = bytes.end.saturating_add(1);
        let mark = one(line, &mut section, rows);
        let value_at = line
            .find('=')
            .filter(|_| matches!(mark, Mark::Set(..)))
            .map(|eq| {
                let rest = line.get(eq.saturating_add(1)..).unwrap_or_default();
                let blanks = rest.len().saturating_sub(rest.trim_start().len());
                bytes
                    .start
                    .saturating_add(eq)
                    .saturating_add(1)
                    .saturating_add(blanks)
            });
        seen.push(Seen {
            bytes,
            value_at,
            mark,
        });
    }
    seen
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::settings::schema::{Choice, Store};

    fn row(key: &str, control: Control, default: SettingValue, value: SettingValue) -> Setting {
        Setting {
            key: SettingKey::new(key),
            group: "Appearance".into(),
            name: key.to_owned().into(),
            description: "".into(),
            control,
            store: Store::App,
            default: Some(default),
            value: Some(value),
        }
    }

    fn density(value: &str) -> Setting {
        let choice = |word: &'static str| Choice {
            label: word.into(),
            value: word.into(),
        };
        let choices = vec![choice("dense"), choice("compact"), choice("roomy")];
        row(
            "appearance.density",
            Control::Select(choices.into()),
            SettingValue::Text("dense".into()),
            SettingValue::Text(value.to_owned().into()),
        )
    }

    fn glass(value: f64) -> Setting {
        let slider = Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: "%".into(),
        };
        row(
            "appearance.glass",
            slider,
            SettingValue::Number(0.2),
            SettingValue::Number(value),
        )
    }

    #[test]
    fn only_the_changed_settings_are_written_in_aligned_sections() {
        let unchanged = row(
            "layout.chat_side",
            Control::Toggle,
            SettingValue::Flag(true),
            SettingValue::Flag(true),
        );
        let rows = [density("compact"), glass(0.6), unchanged];
        assert_eq!(
            render(&rows),
            "# 2 settings differ from the defaults\n\n[appearance]\ndensity = \"compact\"\nglass   = 0.6\n"
        );
    }

    #[test]
    fn a_comment_after_a_value_is_allowed_and_an_unquoted_word_is_marked()
    -> Result<(), Box<dyn Error>> {
        let rows = [density("compact")];
        let seen = read(
            "[appearance]\ndensity = \"roomy\"   # from me\ndensity = roomy\n",
            &rows,
        );
        let marks: Vec<&Mark> = seen.iter().map(|line| &line.mark).collect();
        assert_eq!(marks[0], &Mark::Section);
        assert_eq!(
            marks[1],
            &Mark::Set(
                SettingKey::new("appearance.density"),
                SettingValue::Text("roomy".into())
            )
        );
        assert!(matches!(marks[2], Mark::Bad(why) if why.starts_with("a value is")));
        Ok(())
    }

    #[test]
    fn a_line_naming_nothing_or_a_value_out_of_range_is_marked_by_name() {
        let rows = [glass(0.6)];
        let seen = read("[appearance]\nglas = 1\nglass = 5\n", &rows);
        assert_eq!(
            seen[1].mark,
            Mark::Bad("no setting named appearance.glas".to_owned())
        );
        assert_eq!(
            seen[2].mark,
            Mark::Bad("appearance.glass takes a number from 0 to 1".to_owned())
        );
    }

    #[test]
    fn byte_ranges_hold_with_non_ascii_text_and_point_at_the_value() {
        let rows = [density("compact")];
        let text = "# é\n[appearance]\ndensity =  \"roomy\"\n";
        let seen = read(text, &rows);
        let line = &seen[2];
        assert_eq!(text.get(line.bytes.clone()), Some("density =  \"roomy\""));
        assert_eq!(
            line.value_at.and_then(|at| text.get(at..line.bytes.end)),
            Some("\"roomy\"")
        );
    }
}
