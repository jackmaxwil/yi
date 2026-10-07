use std::io::Read;

pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TAR_BYTES: usize = 256 * 1024 * 1024;

pub fn agent() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
}

pub fn read_capped(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ARCHIVE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_ARCHIVE_BYTES) {
        return Err(format!("larger than {MAX_ARCHIVE_BYTES} bytes"));
    }
    Ok(bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// RFC 1952: a 10-byte header, optional fields named by the flag byte, raw deflate, an
/// 8-byte trailer. Callers authenticate the bytes first, so the CRC is not re-checked.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let bad = || "is not gzip".to_owned();
    if bytes.get(..3) != Some(&[0x1f, 0x8b, 8][..]) {
        return Err(bad());
    }
    let flags = *bytes.get(3).ok_or_else(bad)?;
    let mut at = 10usize;
    if flags & 0x04 != 0 {
        let len = bytes.get(at..at.saturating_add(2)).ok_or_else(bad)?;
        let len = usize::from(u16::from_le_bytes(len.try_into().map_err(|_| bad())?));
        at = at.saturating_add(2).saturating_add(len);
    }
    for name_or_comment in [0x08, 0x10] {
        if flags & name_or_comment != 0 {
            let rest = bytes.get(at..).ok_or_else(bad)?;
            let end = rest.iter().position(|byte| *byte == 0).ok_or_else(bad)?;
            at = at.saturating_add(end).saturating_add(1);
        }
    }
    if flags & 0x02 != 0 {
        at = at.saturating_add(2);
    }
    let deflate = bytes
        .get(at..bytes.len().saturating_sub(8))
        .ok_or_else(bad)?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(deflate, MAX_TAR_BYTES)
        .map_err(|error| format!("does not inflate: {:?}", error.status))
}

pub struct Entry<'a> {
    pub name: String,
    pub regular: bool,
    pub bytes: &'a [u8],
}

/// Names are returned as stored, `..` included: refusing a path is the caller's policy.
pub fn entries(mut tar: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    let mut found = Vec::new();
    while let Some((header, rest)) = tar.split_at_checked(512) {
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = entry_name(header)?;
        let size = octal(header.get(124..136).unwrap_or_default())
            .ok_or_else(|| format!("entry {name} has no size"))?;
        let padded = size
            .div_ceil(512)
            .checked_mul(512)
            .ok_or_else(|| format!("entry {name} is too large"))?;
        let bytes = rest
            .get(..size)
            .ok_or_else(|| format!("entry {name} is truncated"))?;
        let regular = matches!(header.get(156), Some(b'0' | 0));
        found.push(Entry {
            name,
            regular,
            bytes,
        });
        tar = rest.get(padded..).unwrap_or_default();
    }
    Ok(found)
}

fn entry_name(header: &[u8]) -> Result<String, String> {
    let name = header_text(header, 0, 100)?;
    if header.get(257..262) != Some(b"ustar".as_slice()) {
        return Ok(name);
    }
    let prefix = header_text(header, 345, 155)?;
    if prefix.is_empty() {
        Ok(name)
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

fn header_text(header: &[u8], at: usize, len: usize) -> Result<String, String> {
    let raw = header.get(at..at.saturating_add(len)).unwrap_or_default();
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    let bytes = raw.get(..end).unwrap_or_default();
    String::from_utf8(bytes.to_vec()).map_err(|_| "an entry name is not utf-8".to_owned())
}

fn octal(field: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(field).ok()?;
    usize::from_str_radix(text.trim_matches(['\0', ' ']), 8).ok()
}
