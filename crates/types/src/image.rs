//! Whether a provider takes an image: an image it refuses stays in history and fails every later
//! request, so the request side checks each one again, whatever made it and whenever.

use std::fmt;

/// Invariant: the image types and per-image limits of Claude's vision API (docs, 2026-09), which
/// `attach_image` encodes to as well; the other wires take at least as much.
pub const MODEL_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
pub const MAX_IMAGE_CHARS: usize = 10_000_000;
pub const MAX_IMAGE_SIDE: u32 = 8000;

/// Why a provider refuses an image; the text completes "the image ...".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDefect {
    Type,
    TooLarge { chars: usize },
    NotBase64,
    NotItsType,
    NoHeader,
    TooWide { width: u32, height: u32 },
}

impl ImageDefect {
    /// The call that sends the image again, each one run in the kernel venv by a test.
    #[must_use]
    pub fn remedy(self) -> &'static str {
        match self {
            Self::TooLarge { .. } | Self::TooWide { .. } => {
                "If its source file is still on disk, run `print(await attach_image(path))` in ipython; it resizes to fit."
            }
            Self::Type | Self::NotBase64 | Self::NotItsType | Self::NoHeader => {
                "If its source file is still on disk, re-encode it in ipython: `from PIL import Image; Image.open(path).convert('RGB').save(f'{path}.jpg'); print(await attach_image(f'{path}.jpg'))`."
            }
        }
    }
}

impl fmt::Display for ImageDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type => write!(f, "is not png, jpeg, gif or webp"),
            Self::TooLarge { chars } => write!(
                f,
                "is {chars} base64 chars, over MAX_IMAGE_CHARS {MAX_IMAGE_CHARS}"
            ),
            Self::NotBase64 => write!(f, "is not strict base64"),
            Self::NotItsType => write!(f, "does not open as its type"),
            Self::NoHeader => write!(f, "has no whole header for its type"),
            Self::TooWide { width, height } => write!(
                f,
                "is {width}x{height} px, over MAX_IMAGE_SIDE {MAX_IMAGE_SIDE} px a side"
            ),
        }
    }
}

/// What a provider would refuse in this image, or [`None`] when it takes it. Only the first 24
/// bytes are decoded: a body cut short or followed by a trailer is still sent, as decoders read it.
#[must_use]
pub fn image_defect(mime_type: &str, data: &str) -> Option<ImageDefect> {
    if !MODEL_IMAGE_TYPES.contains(&mime_type) {
        return Some(ImageDefect::Type);
    }
    if data.len() > MAX_IMAGE_CHARS {
        return Some(ImageDefect::TooLarge { chars: data.len() });
    }
    let Some(head) = strict_base64_head(data) else {
        return Some(ImageDefect::NotBase64);
    };
    if !has_magic_number(mime_type, &head) {
        return Some(ImageDefect::NotItsType);
    }
    let whole = head.len() == 24
        && match mime_type {
            "image/png" => head.get(8..16) == Some(b"\0\0\0\rIHDR"),
            "image/webp" => matches!(head.get(12..16), Some(b"VP8 " | b"VP8L" | b"VP8X")),
            _ => true,
        };
    if !whole {
        return Some(ImageDefect::NoHeader);
    }
    let u32_at = |at: usize| head.get(at..at.saturating_add(4))?.try_into().ok();
    let u16_at = |at: usize| head.get(at..at.saturating_add(2))?.try_into().ok();
    let side = match mime_type {
        "image/png" => u32_at(16)
            .zip(u32_at(20))
            .map(|(w, h)| (u32::from_be_bytes(w), u32::from_be_bytes(h))),
        "image/gif" => u16_at(6).zip(u16_at(8)).map(|(w, h)| {
            (
                u32::from(u16::from_le_bytes(w)),
                u32::from(u16::from_le_bytes(h)),
            )
        }),
        _ => None,
    };
    match side {
        Some((width, height)) if width.max(height) > MAX_IMAGE_SIDE => {
            Some(ImageDefect::TooWide { width, height })
        }
        _ => None,
    }
}

/// The value of `byte` in the standard base64 alphabet (RFC 4648 §4).
fn sextet(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn unpadded(data: &str) -> &str {
    data.strip_suffix("==")
        .or_else(|| data.strip_suffix('='))
        .unwrap_or(data)
}

/// The bytes `chars` decodes to: whole four-character groups, the last one possibly short.
fn decode_base64(chars: &[u8]) -> Vec<u8> {
    chars
        .chunks(4)
        .flat_map(|group| {
            let bits = group
                .iter()
                .filter_map(|&byte| sextet(byte))
                .fold(0, |acc, value| acc << 6 | value);
            let [_, a, b, c] = (bits << (6 * 4usize.saturating_sub(group.len()))).to_be_bytes();
            [a, b, c].into_iter().take(group.len().saturating_sub(1))
        })
        .collect()
}

/// Whether `data` is strict base64: the standard alphabet in whole four-character groups, `=`
/// only as the last one or two.
#[must_use]
pub fn is_strict_base64(data: &str) -> bool {
    let body = unpadded(data);
    !data.is_empty() && data.len().is_multiple_of(4) && body.bytes().all(|b| sextet(b).is_some())
}

/// The first 24 bytes `data` decodes to, or [`None`] when it is not strict base64.
#[must_use]
pub fn strict_base64_head(data: &str) -> Option<Vec<u8>> {
    let body = unpadded(data).as_bytes();
    is_strict_base64(data).then(|| decode_base64(body.get(..32).unwrap_or(body)))
}

/// Whether `head` opens with the magic number of the provider image type `mime_type` names; any
/// other type has none to check.
#[must_use]
pub fn has_magic_number(mime_type: &str, head: &[u8]) -> bool {
    match mime_type {
        "image/png" => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => head.starts_with(b"\xff\xd8\xff"),
        "image/gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "image/webp" => head.starts_with(b"RIFF") && head.get(8..12) == Some(&b"WEBP"[..]),
        _ => true,
    }
}
