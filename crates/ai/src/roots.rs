use ureq::rustls::RootCertStore;
use ureq::rustls::pki_types::CertificateDer;
use ureq::rustls::pki_types::pem::PemObject;

/// System trust-store PEM for the cursor handshake and the update fetch: the macOS
/// SystemRootCertificates keychain, or the Linux bundle rustls-native-certs does not load.
pub fn system_pem() -> Result<Vec<u8>, String> {
    if cfg!(target_os = "macos") {
        #[expect(
            clippy::disallowed_methods,
            reason = "the system trust store is what the handshake verifies against"
        )]
        let output = std::process::Command::new("security")
            .args([
                "find-certificate",
                "-a",
                "-p",
                "/System/Library/Keychains/SystemRootCertificates.keychain",
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        return Ok(output.stdout);
    }
    std::fs::read("/etc/ssl/certs/ca-certificates.crt")
        .or_else(|_| std::fs::read("/etc/ssl/cert.pem"))
        .map_err(|error| error.to_string())
}

/// PEM trust anchors as a rustls root store; an empty parse is a refusal, never a
/// connection that verifies nothing.
pub fn root_store(pem: &[u8]) -> Result<RootCertStore, String> {
    let ders = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut roots = RootCertStore::empty();
    let (valid, _) = roots.add_parsable_certificates(ders);
    if valid == 0 {
        return Err("the trust store holds no certificates".to_owned());
    }
    Ok(roots)
}
