use std::error::Error;

use sha2::Digest;
use yi_types::encoding::{base64, base64url_nopad};

// Python 3.14 base64.b64encode and urlsafe_b64encode: hex input, standard, url column unpadded.
const PYTHON: &str = include_str!("fixtures/base64-python-v1.tsv");

#[test]
fn both_encoders_match_python_on_every_tail_length() -> Result<(), Box<dyn Error>> {
    for line in PYTHON.lines() {
        let [hex, standard, url] = line.split('\t').collect::<Vec<_>>()[..] else {
            return Err(format!("bad fixture row {line:?}").into());
        };
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(hex.get(at..at + 2).unwrap_or("zz"), 16))
            .collect::<Result<Vec<u8>, _>>()?;
        assert_eq!(base64(&bytes), standard, "standard, input {hex}");
        assert_eq!(base64url_nopad(&bytes), url, "url-safe, input {hex}");
    }
    assert_eq!(PYTHON.lines().count(), 10);
    assert_eq!(base64(b""), "");
    assert_eq!(base64url_nopad(b""), "");
    Ok(())
}

#[test]
fn the_pkce_challenge_matches_rfc_7636_appendix_b() {
    let digest = sha2::Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(
        base64url_nopad(&digest),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}
