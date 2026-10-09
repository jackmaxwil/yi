use std::path::{Path, PathBuf};

pub use yi_tools::plugin::{PLUGINS, Plugin};

pub use yi_kernel::pinned::fetch;

pub enum State {
    Installed,
    /// Versions on disk, none of them the one this yi pins.
    Stale(Vec<String>),
    OnPath(PathBuf),
    Missing,
    Unsupported,
}

pub fn state(plugin: &Plugin, home: &Path) -> State {
    if plugin.installed(home).is_some() {
        return State::Installed;
    }
    let stale = versions(plugin, home);
    if !stale.is_empty() {
        return State::Stale(stale);
    }
    match plugin.found(home) {
        Some(path) => State::OnPath(path),
        None if plugin.asset().is_none() => State::Unsupported,
        None => State::Missing,
    }
}

/// Installs the pinned version, then deletes the older ones; a newer one belongs to a newer yi
/// sharing this home, which would lose its plugin mid-session if this deleted it.
pub fn add(
    plugin: &Plugin,
    home: &Path,
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<PathBuf, String> {
    let path = match plugin.installed(home) {
        Some(path) => path,
        None => {
            let asset = plugin.asset().ok_or_else(|| unsupported(plugin))?;
            let pin = yi_kernel::pinned::Pinned {
                label: &format!("{} {}", plugin.name, plugin.version),
                url: asset.url,
                sha256: asset.sha256,
                member: asset.member,
                dir: &plugin.dir(home),
                file: plugin.name,
            };
            yi_kernel::pinned::install(&pin, fetch)?
        }
    };
    for version in versions(plugin, home) {
        if older(&version, plugin.version) {
            let old = plugin.root(home).join(&version);
            match std::fs::remove_dir_all(&old) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: {error}", old.display()));
                }
                _ => {}
            }
        }
    }
    Ok(path)
}

pub fn remove(plugin: &Plugin, home: &Path) -> Result<bool, String> {
    let root = plugin.root(home);
    match std::fs::remove_dir_all(&root) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", root.display())),
    }
}

/// The plugins the user installed at some version, which `yi plugin sync` brings to the pin.
pub fn chosen(plugin: &Plugin, home: &Path) -> bool {
    plugin.installed(home).is_some() || !versions(plugin, home).is_empty()
}

pub fn unsupported(plugin: &Plugin) -> String {
    format!(
        "yi pins no {} build for {}/{}; build it from source and put it on PATH",
        plugin.name,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Dotted numeric versions compared part by part; anything else is never called older.
fn older(version: &str, than: &str) -> bool {
    let parts =
        |text: &str| -> Option<Vec<u64>> { text.split('.').map(|p| p.parse().ok()).collect() };
    matches!((parts(version), parts(than)), (Some(a), Some(b)) if a < b)
}

/// Published versions other than the pinned one; a staging directory starts with a dot.
fn versions(plugin: &Plugin, home: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(plugin.root(home)) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && name != plugin.version)
        .collect();
    found.sort();
    found
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use yi_tools::plugin::Asset;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// One ustar member in a gzip of stored deflate blocks, which is what a release tarball is
    /// to the unpacker; the digest, not the CRC, is what yi checks.
    fn archive(member: &str, body: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
        let mut tar = vec![0u8; 512];
        tar[..member.len()].copy_from_slice(member.as_bytes());
        tar[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        tar[156] = b'0';
        tar[257..262].copy_from_slice(b"ustar");
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512 + 1024, 0);
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 1];
        let len = u16::try_from(tar.len())?;
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(&tar);
        out.extend([0; 8]);
        Ok(out)
    }

    fn plugin(bytes: &[u8], here: bool) -> Plugin {
        let sha256 = Box::leak(yi_kernel::tarball::sha256_hex(bytes).into_boxed_str());
        let assets: &'static [Asset] = Box::leak(Box::new([Asset {
            os: if here { std::env::consts::OS } else { "plan9" },
            arch: std::env::consts::ARCH,
            url: "https://example.invalid/tool-2.0.tar.gz",
            sha256,
            member: "tool-2.0/tool",
        }]));
        Plugin {
            name: "tool",
            version: "2.0",
            summary: "a test tool",
            card: "tool card",
            assets,
        }
    }

    #[test]
    fn add_publishes_the_pin_and_deletes_the_older_version_it_replaces() -> TestResult {
        let scratch = crate::scratch::Scratch::new("plugin-add")?;
        let home = scratch.home()?;
        let bytes = archive("tool-2.0/tool", b"#!/bin/sh\necho tool\n")?;
        let tool = plugin(&bytes, true);
        assert!(matches!(state(&tool, &home), State::Missing));
        let old = tool.root(&home).join("1.0");
        std::fs::create_dir_all(&old)?;
        std::fs::write(old.join("tool"), "old")?;
        assert!(matches!(state(&tool, &home), State::Stale(v) if v == ["1.0"]));
        assert!(
            chosen(&tool, &home),
            "a stale install is one sync brings forward"
        );
        let newer = tool.root(&home).join("10.0");
        std::fs::create_dir_all(&newer)?;
        let path = add(&tool, &home, |_| Ok(bytes.clone()))?;
        assert!(
            newer.exists(),
            "a newer yi's version is not this yi's to delete"
        );
        assert_eq!(path, tool.dir(&home).join("tool"));
        assert_eq!(std::fs::read(&path)?, b"#!/bin/sh\necho tool\n");
        assert!(!old.exists(), "the replaced version is deleted");
        assert!(matches!(state(&tool, &home), State::Installed));
        assert!(remove(&tool, &home)?);
        assert!(
            !remove(&tool, &home)?,
            "a second remove has nothing to remove"
        );
        assert!(matches!(state(&tool, &home), State::Missing));
        assert!(!chosen(&tool, &home));
        Ok(())
    }

    #[test]
    fn a_tampered_download_or_a_target_with_no_build_installs_nothing() -> TestResult {
        let scratch = crate::scratch::Scratch::new("plugin-refused")?;
        let home = scratch.home()?;
        let tool = plugin(&archive("tool-2.0/tool", b"genuine")?, true);
        let tampered = archive("tool-2.0/tool", b"tampered")?;
        let error = add(&tool, &home, |_| Ok(tampered.clone()))
            .err()
            .ok_or("installed")?;
        assert!(error.contains("not installing"), "{error}");
        let elsewhere = plugin(&tampered, false);
        let error = add(&elsewhere, &home, |_| Err("never asked".to_owned()))
            .err()
            .ok_or("installed")?;
        assert!(
            error.contains("build it from source and put it on PATH"),
            "{error}"
        );
        assert!(matches!(state(&elsewhere, &home), State::Unsupported));
        assert!(!tool.root(&home).join("2.0").exists());
        Ok(())
    }
}
