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

pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let response = crate::tarball::agent()
        .build()
        .get(url)
        .call()
        .map_err(|error| error.to_string())?;
    crate::tarball::read_capped(response.into_reader())
}

/// Downloads the pinned archive through `fetch`, checks its digest before reading a byte of
/// it, and publishes the one `uv` executable it carries; the archive's paths are never used.
pub fn install(
    home: &Path,
    release: &Release,
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<PathBuf, String> {
    if let Some(uv) = installed(home) {
        return Ok(uv);
    }
    let url = release.url();
    let archive = fetch(&url)
        .map_err(|error| format!("couldn't download uv {UV_VERSION}: {error}; {REMEDY}"))?;
    let digest = crate::tarball::sha256_hex(&archive);
    if digest != release.sha256 {
        return Err(format!(
            "{url} has sha256 {digest}, yi pins {}; not installing. {REMEDY}",
            release.sha256
        ));
    }
    let tar =
        crate::tarball::gunzip(&archive).map_err(|error| format!("the uv archive {error}"))?;
    let binary = tar_file(&tar, &format!("uv-{}/uv", release.triple))?;
    let root = home.join(".yi").join("uv");
    let staging = root.join(format!(".staging-{}", std::process::id()));
    let published = publish(&staging, &root.join(UV_VERSION), binary);
    let _ = std::fs::remove_dir_all(&staging);
    published?;
    installed(home).ok_or_else(|| format!("uv {UV_VERSION} published but not executable"))
}

fn publish(staging: &Path, target: &Path, binary: &[u8]) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(at(staging))?;
    let uv = staging.join("uv");
    std::fs::write(&uv, binary).map_err(at(&uv))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).map_err(at(&uv))?;
    }
    // A concurrent installer that won the rename published the same verified bytes.
    match std::fs::rename(staging, target) {
        Ok(()) => Ok(()),
        Err(_) if target.join("uv").is_file() => Ok(()),
        Err(error) => Err(at(target)(error)),
    }
}

fn at(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |error| format!("{}: {error}", path.display())
}

fn tar_file<'a>(tar: &'a [u8], path: &str) -> Result<&'a [u8], String> {
    crate::tarball::entries(tar)
        .map_err(|error| format!("the uv archive: {error}"))?
        .into_iter()
        .find(|entry| entry.regular && entry.name == path)
        .map(|entry| entry.bytes)
        .ok_or_else(|| format!("the uv archive carries no {path}"))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    const TRIPLE: &str = "aarch64-apple-darwin";

    fn tar_entry(out: &mut Vec<u8>, name: &str, typeflag: u8, data: &[u8]) {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[156] = typeflag;
        header[257..262].copy_from_slice(b"ustar");
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }

    fn gzip(tar: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0x08, 0, 0, 0, 0, 0, 3];
        out.extend_from_slice(b"uv.tar\0");
        out.extend(miniz_oxide::deflate::compress_to_vec(tar, 6));
        out.extend_from_slice(&[0; 8]);
        out
    }

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
