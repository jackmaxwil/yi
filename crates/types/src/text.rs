//! The one char-boundary byte cap every layer cuts text with.

/// The head of `text` within `cap` bytes, walked back to a character boundary.
/// Invariant: a byte cap never splits a multi-byte character.
pub fn truncate_utf8(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_back_to_a_character_boundary() {
        assert_eq!(truncate_utf8("héllo", 2), "h");
        assert_eq!(truncate_utf8("héllo", usize::MAX), "héllo");
        assert_eq!(truncate_utf8("", 4), "");
    }
}
