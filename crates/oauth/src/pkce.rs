use std::io::Read;

use sha2::Digest;

/// Secrets-grade randomness (D37): /dev/urandom directly — `rand` is banned
/// and hash-seed tricks are for ids, never for PKCE verifiers or CSRF state.
pub fn random_bytes(count: usize) -> Result<Vec<u8>, String> {
    let mut buffer = vec![0u8; count];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut buffer))
        .map_err(|error| format!("cannot read /dev/urandom: {error}"))?;
    Ok(buffer)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url (RFC 4648 §5) — the encoding RFC 7636 requires for the
/// S256 challenge.
pub fn base64url_nopad(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64URL[(triple >> 18) as usize & 63] as char);
        out.push(BASE64URL[(triple >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL[(triple >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL[triple as usize & 63] as char);
        }
    }
    out
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

pub fn generate() -> Result<Pkce, String> {
    let verifier = hex(&random_bytes(32)?);
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    Ok(Pkce {
        challenge: base64url_nopad(&digest),
        verifier,
        state: hex(&random_bytes(16)?),
    })
}
