use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING: AtomicU64 = AtomicU64::new(0);

pub struct Pinned<'a> {
    pub label: &'a str,
    pub url: &'a str,
    pub sha256: &'a str,
    pub member: &'a str,
    pub dir: &'a Path,
    pub file: &'a str,
}

pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let response = crate::tarball::agent()
        .build()
        .get(url)
        .call()
        .map_err(|error| error.to_string())?;
    crate::tarball::read_capped(response.into_reader())
}

/// Downloads through `fetch`, checks the digest before reading a byte of the archive, and
/// publishes the member as `dir/file` by one directory rename; the archive's paths are never used.
pub fn install(
    pin: &Pinned<'_>,
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<PathBuf, String> {
    let archive =
        fetch(pin.url).map_err(|error| format!("couldn't download {}: {error}", pin.label))?;
    let digest = crate::tarball::sha256_hex(&archive);
    if digest != pin.sha256 {
        return Err(format!(
            "{} has sha256 {digest}, yi pins {}; not installing",
            pin.url, pin.sha256
        ));
    }
    let tar = crate::tarball::gunzip(&archive)
        .map_err(|error| format!("the {} archive {error}", pin.label))?;
    let binary = crate::tarball::entries(&tar)
        .map_err(|error| format!("the {} archive: {error}", pin.label))?
        .into_iter()
        .find(|entry| entry.regular && entry.name == pin.member)
        .map(|entry| entry.bytes)
        .ok_or_else(|| format!("the {} archive carries no {}", pin.label, pin.member))?;
    let parent = pin
        .dir
        .parent()
        .ok_or_else(|| format!("{}: no parent to stage {} in", pin.dir.display(), pin.label))?;
    std::fs::create_dir_all(parent).map_err(at(parent))?;
    let tick = STAGING.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(".staging-{}-{tick}", std::process::id()));
    let published = publish(&staging, pin, binary);
    let _ = std::fs::remove_dir_all(&staging);
    published?;
    let path = pin.dir.join(pin.file);
    crate::bootstrap::is_executable(&path)
        .then_some(path)
        .ok_or_else(|| format!("{} published but not executable", pin.label))
}

fn publish(staging: &Path, pin: &Pinned<'_>, binary: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(staging).map_err(at(staging))?;
    let file = staging.join(pin.file);
    std::fs::write(&file, binary).map_err(at(&file))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
            .map_err(at(&file))?;
    }
    // Incident: the rename can lose to a concurrent installer or find a target written by a
    // poisoned cache, so an existing file is adopted only when it is the verified bytes.
    match std::fs::rename(staging, pin.dir) {
        Ok(()) => Ok(()),
        Err(_)
            if std::fs::read(pin.dir.join(pin.file)).is_ok_and(|existing| existing == binary) =>
        {
            Ok(())
        }
        Err(error) => Err(at(pin.dir)(error)),
    }
}

fn at(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |error| format!("{}: {error}", path.display())
}

#[cfg(test)]
#[cfg(unix)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tar_entry(out: &mut Vec<u8>, name: &str, typeflag: u8, data: &[u8]) {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[156] = typeflag;
        header[257..262].copy_from_slice(b"ustar");
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }

    pub(crate) fn gzip(tar: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0x08, 0, 0, 0, 0, 0, 3];
        out.extend_from_slice(b"uv.tar\0");
        out.extend(miniz_oxide::deflate::compress_to_vec(tar, 6));
        out.extend_from_slice(&[0; 8]);
        out
    }

    #[test]
    fn sixteen_threads_installing_one_pin_publish_it_once_and_leave_no_staging()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = crate::scratch::Scratch::new("pinned-race")?;
        let home = scratch.home()?;
        let mut tar = Vec::new();
        tar_entry(&mut tar, "tool-1.0/tool", b'0', b"#!/bin/sh\necho tool\n");
        tar.extend_from_slice(&[0; 1024]);
        let bytes = gzip(&tar);
        let sha256 = crate::tarball::sha256_hex(&bytes);
        let dir = home.join("plugins/tool/1.0");
        let pin = Pinned {
            label: "tool 1.0",
            url: "https://example.invalid/tool.tar.gz",
            sha256: &sha256,
            member: "tool-1.0/tool",
            dir: &dir,
            file: "tool",
        };
        let results: Vec<Result<PathBuf, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| scope.spawn(|| install(&pin, |_| Ok(bytes.clone()))))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| Err("panicked".to_owned())))
                .collect()
        });
        for result in results {
            assert_eq!(result?, dir.join("tool"));
        }
        assert_eq!(std::fs::read(dir.join("tool"))?, b"#!/bin/sh\necho tool\n");
        let left: Vec<_> = std::fs::read_dir(home.join("plugins/tool"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()?;
        assert_eq!(left, ["1.0"], "no staging left behind");
        Ok(())
    }
}
