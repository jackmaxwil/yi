use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// The scheme set is deliberately open: `https`, `github`, `s3`, `mount` are legal, so an
/// unrecognized scheme lands in the [`Scheme::External`] tail instead of failing the parse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scheme {
    Local,
    Kernel,
    Plan,
    Agent,
    History,
    Checkpoint,
    Mcp,
    User,
    External(String),
}

/// Ephemeral schemes are legal in live coordination and illegal in a record outliving its
/// referent; the step table matches on this to make that combination unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    Ephemeral,
    Durable,
}

impl Scheme {
    fn parse(text: &str) -> Self {
        match text {
            "local" => Self::Local,
            "kernel" => Self::Kernel,
            "plan" => Self::Plan,
            "agent" => Self::Agent,
            "history" => Self::History,
            "checkpoint" => Self::Checkpoint,
            "mcp" => Self::Mcp,
            "user" => Self::User,
            other => Self::External(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Kernel => "kernel",
            Self::Plan => "plan",
            Self::Agent => "agent",
            Self::History => "history",
            Self::Checkpoint => "checkpoint",
            Self::Mcp => "mcp",
            Self::User => "user",
            Self::External(text) => text,
        }
    }

    /// A live agent or kernel variable dies with its owner; every other
    /// referent survives it.
    pub fn durability(&self) -> Durability {
        match self {
            Self::Kernel | Self::Agent => Durability::Ephemeral,
            Self::Local
            | Self::Plan
            | Self::History
            | Self::Checkpoint
            | Self::Mcp
            | Self::User
            | Self::External(_) => Durability::Durable,
        }
    }

    fn takes_fragment(&self) -> bool {
        matches!(self, Self::Local | Self::Checkpoint)
    }
}

impl std::fmt::Display for Scheme {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A `#L<start>-<end>[@<tag>]` span on a `local://` or `checkpoint://` reference; without a
/// tag it names the current content of those lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HashlineFragment {
    start: NonZeroU32,
    end: NonZeroU32,
    tag: Option<u16>,
}

impl HashlineFragment {
    pub fn new(start: NonZeroU32, end: NonZeroU32, tag: Option<u16>) -> Result<Self, UrlError> {
        if start > end {
            return Err(UrlError::InvertedRange {
                start: start.get(),
                end: end.get(),
            });
        }
        Ok(Self { start, end, tag })
    }

    /// Invariant: the tag is a whole-file xxh32 as exactly four UPPERCASE hex digits — it
    /// pins the file, not the span, and lowercase hex does not parse.
    fn parse(text: &str) -> Result<Self, UrlError> {
        let syntax = || UrlError::FragmentSyntax {
            fragment: text.to_owned(),
        };
        let body = text.strip_prefix('L').ok_or_else(syntax)?;
        let (range, tag_text) = match body.split_once('@') {
            Some((range, tag_text)) => (range, Some(tag_text)),
            None => (body, None),
        };
        let (start_text, end_text) = range.split_once('-').ok_or_else(syntax)?;
        let start = parse_line(start_text, text)?;
        let end = parse_line(end_text, text)?;
        let tag = tag_text
            .map(|tag_text| {
                let bad = || UrlError::TagSyntax {
                    tag: tag_text.to_owned(),
                };
                if tag_text.len() != 4
                    || !tag_text
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
                {
                    return Err(bad());
                }
                u16::from_str_radix(tag_text, 16).map_err(|_| bad())
            })
            .transpose()?;
        Self::new(start, end, tag)
    }

    pub fn start(&self) -> NonZeroU32 {
        self.start
    }

    pub fn end(&self) -> NonZeroU32 {
        self.end
    }

    pub fn tag(&self) -> Option<u16> {
        self.tag
    }
}

fn parse_line(text: &str, fragment: &str) -> Result<NonZeroU32, UrlError> {
    let value: u32 = text.parse().map_err(|_| UrlError::FragmentSyntax {
        fragment: fragment.to_owned(),
    })?;
    NonZeroU32::new(value).ok_or(UrlError::ZeroLine {
        fragment: fragment.to_owned(),
    })
}

impl std::fmt::Display for HashlineFragment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "L{}-{}", self.start, self.end)?;
        match self.tag {
            Some(tag) => write!(formatter, "@{tag:04X}"),
            None => Ok(()),
        }
    }
}

/// The one reference type: every addressable thing a plan, delegation, or
/// record can point at is this URL, serialized as the flat string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Url {
    scheme: Scheme,
    path: String,
    fragment: Option<HashlineFragment>,
}

impl Url {
    pub fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn fragment(&self) -> Option<&HashlineFragment> {
        self.fragment.as_ref()
    }

    pub fn durability(&self) -> Durability {
        self.scheme.durability()
    }
}

impl std::str::FromStr for Url {
    type Err = UrlError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.chars().any(char::is_whitespace) {
            return Err(UrlError::Whitespace {
                url: text.to_owned(),
            });
        }
        let (scheme_text, rest) = text.split_once("://").ok_or_else(|| UrlError::NoScheme {
            url: text.to_owned(),
        })?;
        if scheme_text.is_empty() {
            return Err(UrlError::EmptyScheme {
                url: text.to_owned(),
            });
        }
        let scheme = Scheme::parse(scheme_text);
        let (path, fragment_text) = match rest.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (rest, None),
        };
        if path.is_empty() {
            return Err(UrlError::EmptyPath {
                url: text.to_owned(),
            });
        }
        let fragment = match fragment_text {
            Some(_) if !scheme.takes_fragment() => {
                return Err(UrlError::FragmentNotAllowed {
                    url: text.to_owned(),
                    scheme: scheme.as_str().to_owned(),
                });
            }
            Some(fragment) => Some(HashlineFragment::parse(fragment)?),
            None => None,
        };
        Ok(Self {
            scheme,
            path: path.to_owned(),
            fragment,
        })
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}://{}", self.scheme, self.path)?;
        match &self.fragment {
            Some(fragment) => write!(formatter, "#{fragment}"),
            None => Ok(()),
        }
    }
}

impl TryFrom<String> for Url {
    type Error = UrlError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Url> for String {
    fn from(url: Url) -> Self {
        url.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    NoScheme { url: String },
    EmptyScheme { url: String },
    EmptyPath { url: String },
    Whitespace { url: String },
    FragmentNotAllowed { url: String, scheme: String },
    FragmentSyntax { fragment: String },
    ZeroLine { fragment: String },
    InvertedRange { start: u32, end: u32 },
    TagSyntax { tag: String },
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScheme { url } => write!(formatter, "url {url:?} has no scheme separator"),
            Self::EmptyScheme { url } => write!(formatter, "url {url:?} has an empty scheme"),
            Self::EmptyPath { url } => write!(formatter, "url {url:?} has an empty path"),
            Self::Whitespace { url } => write!(formatter, "url {url:?} contains whitespace"),
            Self::FragmentNotAllowed { url, scheme } => write!(
                formatter,
                "url {url:?} carries a fragment but scheme {scheme} takes none"
            ),
            Self::FragmentSyntax { fragment } => write!(
                formatter,
                "fragment {fragment:?} is not L<start>-<end>@<tag> or L<start>-<end>"
            ),
            Self::ZeroLine { fragment } => {
                write!(formatter, "fragment {fragment:?} names line zero")
            }
            Self::InvertedRange { start, end } => {
                write!(formatter, "line range {start}-{end} is inverted")
            }
            Self::TagSyntax { tag } => {
                write!(formatter, "tag {tag:?} is not four uppercase hex digits")
            }
        }
    }
}

impl std::error::Error for UrlError {}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn round_trips_and_classifies() -> TestResult {
        let url: Url = "local://src/auth.rs#L42-58@9F3E".parse()?;
        assert_eq!(url.scheme(), &Scheme::Local);
        assert_eq!(url.path(), "src/auth.rs");
        let fragment = url.fragment().ok_or("fragment missing")?;
        assert_eq!(fragment.start().get(), 42);
        assert_eq!(fragment.tag(), Some(0x9F3E));
        assert_eq!(url.to_string(), "local://src/auth.rs#L42-58@9F3E");
        assert_eq!(url.durability(), Durability::Durable);
        let agent: Url = "agent://main".parse()?;
        assert_eq!(agent.durability(), Durability::Ephemeral);
        let external: Url = "https://example.com/page".parse()?;
        assert_eq!(external.scheme(), &Scheme::External("https".to_owned()));
        Ok(())
    }

    #[test]
    fn a_fragment_without_a_tag_parses_and_round_trips() -> TestResult {
        let url: Url = "local://src/auth.rs#L42-58".parse()?;
        let fragment = url.fragment().ok_or("fragment missing")?;
        assert_eq!((fragment.start().get(), fragment.end().get()), (42, 58));
        assert_eq!(fragment.tag(), None);
        assert_eq!(url.to_string(), "local://src/auth.rs#L42-58");
        let one: Url = "checkpoint://abc/x#L7-7".parse()?;
        assert_eq!(one.to_string(), "checkpoint://abc/x#L7-7");
        let max: Url = "local://x#L1-4294967295@FFFF".parse()?;
        assert_eq!(max.to_string(), "local://x#L1-4294967295@FFFF");
        Ok(())
    }

    #[test]
    fn the_syntax_error_names_both_forms() {
        let error = "local://x#L5".parse::<Url>().err().map(|e| e.to_string());
        let text = error.unwrap_or_default();
        assert!(text.contains("L<start>-<end>@<tag>"), "{text}");
        assert!(
            text.replace("L<start>-<end>@<tag>", "")
                .contains("L<start>-<end>"),
            "{text}"
        );
    }

    #[test]
    fn rejects_at_construction() -> TestResult {
        for bad in [
            "noscheme",
            "://x",
            "local://",
            "local://a b",
            "plan://p#L1-2@9F3E",
            "local://x#L42-58@a3f2",
            "local://x#L0-2@9F3E",
            "local://x#L5-2@9F3E",
            "local://x#L0-2",
            "local://x#L5-2",
            "local://x#L1-2@",
            "local://x#L1-2@9F3",
            "local://x#L1-2@9F3E0",
            "local://x#L1-@9F3E",
            "plan://p#L1-2",
        ] {
            assert!(bad.parse::<Url>().is_err(), "{bad} parsed");
        }
        Ok(())
    }
}
