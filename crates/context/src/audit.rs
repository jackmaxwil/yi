use std::collections::BTreeSet;

use yi_types::message::AgentMessage;

use crate::serialize::serialize_conversation;

pub const DROPPED_CAP: usize = 40;
pub const PINNED_CAP: usize = 32;
pub const CONSTRAINT_MARKERS: &[&str] = &["must", "never", "don't", "dont", "always", "only"];

pub fn is_constraint_line(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    CONSTRAINT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

pub fn identifiers(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if is_ident_char(ch) {
            buf.push(ch);
        } else {
            flush(&mut buf, &mut out);
        }
    }
    flush(&mut buf, &mut out);
    out
}

pub fn message_text(message: &AgentMessage) -> String {
    serialize_conversation(std::slice::from_ref(message))
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '/' | '.' | ':' | '#' | '-')
}

fn flush(buf: &mut String, out: &mut BTreeSet<String>) {
    if keep(buf) {
        out.insert(std::mem::take(buf));
    } else {
        buf.clear();
    }
}

fn keep(tok: &str) -> bool {
    if tok.chars().count() < 2 {
        return false;
    }
    if tok.contains("://") {
        return true;
    }
    if tok.contains('/') {
        return true;
    }
    if rustc_error(tok) || issue_number(tok) || file_ext(tok) {
        return true;
    }
    if tok.contains('_') && tok.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return true;
    }
    camel_case(tok)
}

fn rustc_error(tok: &str) -> bool {
    let mut chars = tok.chars();
    if chars.next() != Some('E') {
        return false;
    }
    let rest: String = chars.collect();
    rest.len() >= 4 && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn issue_number(tok: &str) -> bool {
    let mut chars = tok.chars();
    if chars.next() != Some('#') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn file_ext(tok: &str) -> bool {
    let Some((stem, ext)) = tok.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !ext.is_empty()
        && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
        && stem.chars().any(|ch| ch.is_ascii_alphanumeric())
}

fn camel_case(tok: &str) -> bool {
    let mut seen_lower = false;
    for ch in tok.chars() {
        if ch.is_ascii_lowercase() {
            seen_lower = true;
        } else if seen_lower && ch.is_ascii_uppercase() {
            return true;
        }
    }
    false
}
