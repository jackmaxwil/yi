use std::path::{Path, PathBuf};

/// Bump the version and all four digests together, from the release's `.sha256` files.
pub const UV_VERSION: &str = "0.12.18";
const PINNED: [(&str, &str, &str, &str); 4] = [
    (
        "macos",
        "aarch64",
        "aarch64-apple-darwin",
        "cf40e0c6a202190ccd9e0406dcfdd5b2d6668a9a5c779b17948963df32aafe5b",
    ),
    (
        "macos",
        "x86_64",
        "x86_64-apple-darwin",
        "2e4108f5395397c8bc5d43bf83d3bdbb2d0e92b90d0efa607756be704905fa33",
    ),
    (
        "linux",
        "aarch64",
        "aarch64-unknown-linux-musl",
        "0796973fb3eea8095078c3d0659bd17a5f6789a71b8dd85caff2483178f78ac3",
    ),
    (
        "linux",
        "x86_64",
        "x86_64-unknown-linux-musl",
        "e38d97460b98ebfd31b197de0fe9fa578add4bc8ba0179b203dd3f87b99f98e6",
    ),
];
const REMEDY: &str = "install uv (https://docs.astral.sh/uv/) or python3-venv, then re-run yi";

pub struct Release {
    pub triple: &'static str,
    pub sha256: &'static str,
}

impl Release {
    pub fn pinned() -> Result<Self, String> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        PINNED
            .iter()
            .find(|(o, a, _, _)| *o == os && *a == arch)
            .map(|(_, _, triple, sha256)| Self { triple, sha256 })
            .ok_or_else(|| format!("yi pins no uv build for {os}/{arch}; {REMEDY}"))
    }

    fn url(&self) -> String {
        format!(
            "https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-{}.tar.gz",
            self.triple
        )
    }
}

/// Invariant: only [`install`] writes under `~/.yi/uv/<version>`, and it publishes by one
/// directory rename after the digest matched, so an executable there is a verified install.
pub fn installed(home: &Path) -> Option<PathBuf> {
    let uv = home.join(".yi").join("uv").join(UV_VERSION).join("uv");
    crate::bootstrap::is_executable(&uv).then_some(uv)
}

/// Downloads the pinned archive, checks its digest before reading a byte of it, and publishes
/// the one `uv` executable it carries ([`crate::pinned::install`]).
pub fn install(
    home: &Path,
    release: &Release,
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<PathBuf, String> {
    if let Some(uv) = installed(home) {
        return Ok(uv);
    }
    let pin = crate::pinned::Pinned {
        label: &format!("uv {UV_VERSION}"),
        url: &release.url(),
        sha256: release.sha256,
        member: &format!("uv-{}/uv", release.triple),
        dir: &home.join(".yi").join("uv").join(UV_VERSION),
        file: "uv",
    };
    crate::pinned::install(&pin, fetch).map_err(|error| format!("{error}; {REMEDY}"))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::pinned::tests::{gzip, tar_entry};

    const TRIPLE: &str = "aarch64-apple-darwin";

    fn archive(binary: &[u8]) -> Vec<u8> {
        let mut tar = Vec::new();
        tar_entry(&mut tar, &format!("uv-{TRIPLE}/"), b'5', b"");
        tar_entry(&mut tar, &format!("uv-{TRIPLE}/uvx"), b'0', b"not uv");
        tar_entry(&mut tar, "../../escape", b'0', b"never written");
        tar_entry(&mut tar, &format!("uv-{TRIPLE}/uv"), b'0', binary);
        tar.extend_from_slice(&[0; 1024]);
        gzip(&tar)
    }

    fn release_for(archive: &[u8]) -> Release {
        let sha256 = Box::leak(crate::tarball::sha256_hex(archive).into_boxed_str());
        Release {
            triple: TRIPLE,
            sha256,
        }
    }

    #[test]
    fn a_verified_archive_publishes_only_its_uv_and_is_reused_offline()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("uv-install-ok")?;
        let home = scratch.home()?;
        let bytes = archive(b"#!/bin/sh\necho uv\n");
        let release = release_for(&bytes);
        let uv = install(&home, &release, |_| Ok(bytes.clone()))?;
        assert_eq!(std::fs::read(&uv)?, b"#!/bin/sh\necho uv\n");
        assert!(crate::bootstrap::is_executable(&uv));
        assert!(!home.join("escape").exists() && !home.join(".yi/escape").exists());
        let names: Vec<_> = std::fs::read_dir(home.join(".yi/uv"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()?;
        assert_eq!(names, [UV_VERSION], "no staging left behind");
        let again = install(&home, &release, |_| Err("offline".to_owned()))?;
        assert_eq!(again, uv);
        Ok(())
    }

    #[test]
    fn a_tampered_archive_is_refused_and_nothing_is_published()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("uv-install-tampered")?;
        let home = scratch.home()?;
        let release = release_for(&archive(b"genuine"));
        let error = match install(&home, &release, |_| Ok(archive(b"tampered"))) {
            Ok(path) => return Err(format!("installed {}", path.display()).into()),
            Err(error) => error,
        };
        assert!(error.contains("not installing"), "{error}");
        assert!(installed(&home).is_none());
        Ok(())
    }

    #[test]
    fn an_archive_without_uv_names_the_missing_path() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("uv-install-missing")?;
        let home = scratch.home()?;
        let mut tar = Vec::new();
        tar_entry(&mut tar, &format!("uv-{TRIPLE}/uv"), b'2', b"");
        tar.extend_from_slice(&[0; 1024]);
        let bytes = gzip(&tar);
        let release = release_for(&bytes);
        let error = match install(&home, &release, |_| Ok(bytes.clone())) {
            Ok(path) => return Err(format!("installed {}", path.display()).into()),
            Err(error) => error,
        };
        assert!(error.contains(&format!("no uv-{TRIPLE}/uv")), "{error}");
        Ok(())
    }
}
