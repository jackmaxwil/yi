use std::io::Read;

use sha2::Digest;
use yi_types::encoding::base64url_nopad;

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
