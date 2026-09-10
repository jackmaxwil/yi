use std::io::Read;

use sha2::Digest;

const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn generate() -> Result<Pkce, String> {
    let mut entropy = [0u8; 48];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut entropy))
        .map_err(|error| format!("cannot read /dev/urandom: {error}"))?;
    let verifier = hex(&entropy[..32]);
    let state = hex(&entropy[32..]);
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    Ok(Pkce {
        challenge: url64(&digest),
        verifier,
        state,
    })
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        out.push_str(&format!("{byte:02x}"));
        out
    })
}

fn url64(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let remain = bytes.len() - index;
        let b0 = u32::from(bytes[index]);
        let b1 = u32::from(bytes.get(index + 1).copied().unwrap_or(0));
        let b2 = u32::from(bytes.get(index + 2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(URL_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(URL_ALPHABET[(n >> 12) as usize & 63] as char);
        if remain > 1 {
            out.push(URL_ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if remain > 2 {
            out.push(URL_ALPHABET[n as usize & 63] as char);
        }
        index += 3;
    }
    out
}
