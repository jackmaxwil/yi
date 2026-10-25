const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Standard padded base64 (RFC 4648 §4), the form OSC 52, kitty graphics and ACP carry.
#[must_use]
#[inline]
pub fn base64(bytes: &[u8]) -> String {
    encode(bytes, STANDARD, true)
}

/// Unpadded base64url (RFC 4648 §5), the form a PKCE S256 challenge and a JWT segment take.
#[must_use]
#[inline]
pub fn base64url_nopad(bytes: &[u8]) -> String {
    encode(bytes, URL_SAFE, false)
}

/// Invariant: inlined so the orb's per-frame call compiles at yi-orb's opt-level 3, not this
/// crate's dist `z`, which ran per-frame orb code several times slower.
#[inline]
fn encode(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let byte = |n: usize| u32::from(chunk.get(n).copied().unwrap_or(0));
        let packed = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for shift in [18u32, 12, 6, 0]
            .into_iter()
            .take(chunk.len().saturating_add(1))
        {
            let symbol = alphabet.get(((packed >> shift) & 63) as usize);
            out.push(char::from(symbol.copied().unwrap_or(b'A')));
        }
        if pad {
            out.extend(std::iter::repeat_n('=', 3usize.saturating_sub(chunk.len())));
        }
    }
    out
}
