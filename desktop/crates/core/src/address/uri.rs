//! `yi://<kind>/<stable id>[#part]`: built from record ids, never from display text.

use std::fmt;
use std::str::FromStr;

use crate::kind::{Kind, RESERVED};

/// The longest address accepted, in bytes.
pub const MAX_LEN: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("an address starts with yi://")]
    NoScheme,
    #[error("an address names a kind and an id: yi://<kind>/<id>")]
    NoId,
    #[error("no thing has this kind")]
    UnknownKind,
    #[error("yi://tools and yi://skills belong to Yi")]
    Reserved,
    #[error("an escape is % and two hex digits of valid UTF-8")]
    BadEscape,
    #[error("an address of {len} bytes is over the limit of {max}")]
    TooLong { len: usize, max: usize },
}

/// Where a thing lives. Two things are the same thing exactly when their addresses are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    kind: Kind,
    id: String,
    part: Option<String>,
}

impl Address {
    /// The address of the record `id` of `kind`. The id is a record's own, such as a path.
    pub fn new(kind: Kind, id: impl Into<String>) -> Result<Self, AddressError> {
        let id = id.into();
        if id.is_empty() {
            return Err(AddressError::NoId);
        }
        let address = Self {
            kind,
            id,
            part: None,
        };
        address.fits("")?;
        Ok(address)
    }

    /// The same thing narrowed to a part of it, such as `L13-L20`. An empty part, or one that
    /// would take the address past [`MAX_LEN`], narrows nothing: it stays the whole thing.
    pub fn part(self, part: impl Into<String>) -> Self {
        let part = part.into();
        if part.is_empty() || self.fits(&part).is_err() {
            return self;
        }
        Self {
            part: Some(part),
            ..self
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn part_text(&self) -> Option<&str> {
        self.part.as_deref()
    }

    /// Whether this address, narrowed to `part`, is written in at most [`MAX_LEN`] bytes.
    fn fits(&self, part: &str) -> Result<(), AddressError> {
        let head = "yi://".len().saturating_add(self.kind.slug().len());
        let id = written(&self.id).saturating_add(1);
        let part = match part {
            "" => 0,
            part => written(part).saturating_add(1),
        };
        let len = head.saturating_add(id).saturating_add(part);
        if len > MAX_LEN {
            return Err(AddressError::TooLong { len, max: MAX_LEN });
        }
        Ok(())
    }
}

/// Direction marks and overrides: format characters `is_control` does not cover, which can
/// make a printed address read as another.
fn is_bidi(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The characters written `%XX`, so an address is one word and `#` only starts the part.
fn escaped(c: char) -> bool {
    c == '%' || c == '#' || c.is_whitespace() || c.is_control() || is_bidi(c)
}

/// How many bytes [`escape`] writes for `text`: three for each byte of an escaped character.
fn written(text: &str) -> usize {
    let bytes = |c: char| c.len_utf8().saturating_mul(if escaped(c) { 3 } else { 1 });
    text.chars()
        .fold(0usize, |sum, c| sum.saturating_add(bytes(c)))
}

fn escape(text: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for c in text.chars() {
        if escaped(c) {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                write!(f, "%{byte:02X}")?;
            }
        } else {
            write!(f, "{c}")?;
        }
    }
    Ok(())
}

fn unescape(text: &str) -> Result<String, AddressError> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.bytes();
    while let Some(byte) = rest.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let hex = [rest.next(), rest.next()];
        let [Some(high), Some(low)] =
            hex.map(|digit| digit.and_then(|d| char::from(d).to_digit(16)))
        else {
            return Err(AddressError::BadEscape);
        };
        bytes.push(u8::try_from(high * 16 + low).map_err(|_| AddressError::BadEscape)?);
    }
    String::from_utf8(bytes).map_err(|_| AddressError::BadEscape)
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "yi://{}/", self.kind.slug())?;
        escape(&self.id, f)?;
        if let Some(part) = &self.part {
            f.write_str("#")?;
            escape(part, f)?;
        }
        Ok(())
    }
}

impl FromStr for Address {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, AddressError> {
        if text.len() > MAX_LEN {
            return Err(AddressError::TooLong {
                len: text.len(),
                max: MAX_LEN,
            });
        }
        let rest = text.strip_prefix("yi://").ok_or(AddressError::NoScheme)?;
        let (slug, rest) = rest.split_once('/').ok_or(AddressError::NoId)?;
        if RESERVED.contains(&slug) {
            return Err(AddressError::Reserved);
        }
        let kind = Kind::from_slug(slug).ok_or(AddressError::UnknownKind)?;
        let (id, part) = match rest.split_once('#') {
            Some((id, part)) => (id, unescape(part)?),
            None => (rest, String::new()),
        };
        let address = Address::new(kind, unescape(id)?)?;
        // Invariant: a pasted address is held to the limit as it is written back, not as typed.
        address.fits(&part)?;
        Ok(address.part(part))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    #[test]
    fn an_address_made_in_the_app_is_held_to_the_limit_a_pasted_one_is() -> Check {
        let fits = Address::new(Kind::File, "a".repeat(MAX_LEN - 10))?;
        assert_eq!(fits.to_string().len(), MAX_LEN);
        let long = Address::new(Kind::File, "a".repeat(MAX_LEN - 9));
        let over = AddressError::TooLong {
            len: MAX_LEN + 1,
            max: MAX_LEN,
        };
        assert_eq!(long, Err(over), "one byte over");
        // A space is three bytes once written, so 700 of them are 2,100.
        let spaced = Address::new(Kind::File, " ".repeat(700));
        assert!(matches!(spaced, Err(AddressError::TooLong { .. })));
        Ok(())
    }

    #[test]
    fn a_part_that_does_not_fit_leaves_the_address_whole() -> Check {
        let file = Address::new(Kind::Lines, "a".repeat(MAX_LEN - 20))?;
        let narrowed = file.clone().part("L13-L20");
        assert_eq!(narrowed.part_text(), Some("L13-L20"), "nine bytes fit");
        let whole = file.clone().part("L".repeat(40));
        assert_eq!(whole, file, "a part past the limit narrows nothing");
        assert!(whole.to_string().len() <= MAX_LEN);
        assert_eq!(whole.to_string().parse::<Address>()?, whole);
        Ok(())
    }

    #[test]
    fn an_empty_part_is_no_part() -> Check {
        let whole = Address::new(Kind::File, "src/lib.rs")?;
        assert_eq!(whole.clone().part(""), whole);
        assert_eq!("yi://file/src/lib.rs#".parse::<Address>()?, whole);
        assert_eq!(whole.to_string(), "yi://file/src/lib.rs");
        Ok(())
    }

    #[test]
    fn a_pasted_address_that_would_be_written_past_the_limit_is_refused() {
        // 2,048 bytes as pasted; each raw space is written back as three.
        let pasted = format!("yi://file/{}", " ".repeat(MAX_LEN - 10));
        assert_eq!(pasted.len(), MAX_LEN);
        let parsed = pasted.parse::<Address>();
        assert!(
            matches!(parsed, Err(AddressError::TooLong { max: MAX_LEN, .. })),
            "{parsed:?}"
        );
        let part = format!("yi://file/a#{}", " ".repeat(MAX_LEN - 12));
        assert!(matches!(
            part.parse::<Address>(),
            Err(AddressError::TooLong { .. })
        ));
    }

    #[test]
    fn an_escape_must_be_two_hex_digits_of_whole_characters() {
        for broken in ["%", "%4", "%G1", "%FF", "%C3", "a%2", "%E2%80"] {
            let parsed = format!("yi://file/{broken}").parse::<Address>();
            assert_eq!(parsed, Err(AddressError::BadEscape), "{broken}");
            let part = format!("yi://file/a#{broken}").parse::<Address>();
            assert_eq!(part, Err(AddressError::BadEscape), "part {broken}");
        }
    }

    #[test]
    fn a_direction_mark_is_printed_escaped_so_it_cannot_reverse_the_address() -> Check {
        let address = Address::new(Kind::File, "a\u{202E}b")?;
        let written = address.to_string();
        assert!(written.contains("%E2%80%AE"), "{written}");
        assert!(!written.contains('\u{202E}'), "{written}");
        assert_eq!(written.parse::<Address>()?, address);
        Ok(())
    }
}
