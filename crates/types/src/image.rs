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

/// The ipython statements that send an image at `path` again: `attach_image` resizes one to fit,
/// and Pillow re-encodes one of a type `attach_image` does not take; each is run by a kernel test.
pub const RESIZE_CELL: &str = "print(await attach_image(path))";
pub const REENCODE_CELL: &str = "import tempfile; from PIL import Image; out = tempfile.NamedTemporaryFile(suffix='.png', delete=False).name; Image.open(path).convert('RGBA').save(out); print(await attach_image(out))";

impl ImageDefect {
    /// The statement that sends the image again, `path` naming its source file.
    #[must_use]
    pub fn cell(self) -> &'static str {
        match self {
            Self::TooLarge { .. } | Self::TooWide { .. } => RESIZE_CELL,
            Self::Type | Self::NotBase64 | Self::NotItsType | Self::NoHeader => REENCODE_CELL,
        }
    }

    /// [`ImageDefect::cell`] as the sentence a refusal ends with.
    #[must_use]
    pub fn remedy(self) -> String {
        match self {
            Self::TooLarge { .. } | Self::TooWide { .. } => format!(
                "If its source file is still on disk, run `{}` in ipython; it resizes to fit.",
                self.cell()
            ),
            Self::Type | Self::NotBase64 | Self::NotItsType | Self::NoHeader => format!(
                "If its source file is still on disk, re-encode it in ipython: `{}`.",
                self.cell()
            ),
        }
    }
}

/// The ipython statement that attaches paths expressed as Python arguments.
#[must_use]
pub fn attach_image_call<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
    format!(
        "print(await attach_image({}))",
        paths.into_iter().collect::<Vec<_>>().join(", ")
    )
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
    match image_side(mime_type, &head) {
        Some((width, height)) if width.max(height) > MAX_IMAGE_SIDE => {
            Some(ImageDefect::TooWide { width, height })
        }
        _ => None,
    }
}

/// The width and height an image's header names, read from `bytes` as far as they go: PNG and
/// GIF in their first 24 bytes, WebP in its first 30, JPEG at its first frame marker.
#[must_use]
pub fn image_side(mime_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    let be = |at, len| int(bytes, at, len, true);
    let le = |at, len| int(bytes, at, len, false);
    let plus_one = |side: u32| side.saturating_add(1);
    match mime_type {
        "image/png" => be(16, 4).zip(be(20, 4)),
        "image/gif" => le(6, 2).zip(le(8, 2)),
        "image/webp" => match bytes.get(12..16)? {
            b"VP8 " if bytes.get(23..26) == Some(&[0x9d, 0x01, 0x2a]) => {
                Some((le(26, 2)? & 0x3fff, le(28, 2)? & 0x3fff))
            }
            b"VP8L" if bytes.get(20) == Some(&0x2f) => {
                let bits = le(21, 4)?;
                Some((plus_one(bits & 0x3fff), plus_one(bits >> 14 & 0x3fff)))
            }
            b"VP8X" => Some((plus_one(le(24, 3)?), plus_one(le(27, 3)?))),
            _ => None,
        },
        "image/jpeg" => jpeg_side(bytes),
        _ => None,
    }
}

/// The `len`-byte unsigned integer at `at`, big- or little-endian; `len` is at most 4.
fn int(bytes: &[u8], at: usize, len: usize, big: bool) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(len)?)?;
    let fold = |value: u32, byte: &u8| value << 8 | u32::from(*byte);
    Some(match big {
        true => field.iter().fold(0, fold),
        false => field.iter().rev().fold(0, fold),
    })
}

/// A JPEG's size from its first start-of-frame segment, walking the segments after SOI and, as
/// libjpeg does, past stray bytes to the next marker; [`None`] once the scan or the end comes first.
fn jpeg_side(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2_usize;
    loop {
        let skipped = bytes.get(at..)?.iter().position(|&byte| byte == 0xff)?;
        at = at.checked_add(skipped)?;
        while bytes.get(at) == Some(&0xff) {
            at = at.checked_add(1)?;
        }
        let marker = *bytes.get(at)?;
        match marker {
            0x00 | 0x01 | 0xd0..=0xd7 => at = at.checked_add(1)?,
            0xd9 | 0xda => return None,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return int(bytes, at.checked_add(6)?, 2, true).zip(int(
                    bytes,
                    at.checked_add(4)?,
                    2,
                    true,
                ));
            }
            _ => {
                let len = usize::try_from(int(bytes, at.checked_add(1)?, 2, true)?).ok()?;
                at = at.checked_add(1)?.checked_add(len)?;
            }
        }
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
