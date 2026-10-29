//! `scripts/install.sh` run the way the one-liner runs it: no release tree beside the script,
//! so it downloads from `YI_RELEASE_BASE` and installs only what SHA256SUMS vouches for.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");

/// The triple install.sh names this machine by, which package.sh puts in the archive name.
fn triple() -> Result<String, Box<dyn Error>> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => return Err(format!("no release triple for {other}").into()),
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        other => return Err(format!("no release triple for {other}").into()),
    };
    Ok(format!("{arch}-{os}"))
}

/// One archive in package.sh's layout, whose `bin/yi` prints `says` for `--version`.
#[expect(
    clippy::disallowed_methods,
    reason = "the archive is what `tar -czf` writes, as package.sh writes it"
)]
fn archive(work: &Path, name: &str, says: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let tree = work.join(name);
    std::fs::create_dir_all(tree.join("bin"))?;
    std::fs::create_dir_all(tree.join("python/yi_runtime"))?;
    std::fs::create_dir_all(tree.join("skills/yi/pr-review"))?;
    std::fs::write(tree.join("bin/yi"), format!("#!/bin/sh\necho '{says}'\n"))?;
    std::fs::write(tree.join("python/yi_runtime/__init__.py"), "")?;
    std::fs::write(
        tree.join("skills/yi/pr-review/SKILL.md"),
        "---\nname: pr-review\n---\n",
    )?;
    std::fs::set_permissions(tree.join("bin/yi"), std::fs::Permissions::from_mode(0o755))?;
    let out = work.join("out.tar.gz");
    let tar = Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(work)
        .arg(name)
        .status()?;
    if !tar.success() {
        return Err("could not build the archive".into());
    }
    let bytes = std::fs::read(&out)?;
    std::fs::remove_dir_all(&tree)?;
    std::fs::remove_file(&out)?;
    Ok(bytes)
}

/// A release directory holding `served` under the archive name, with a SHA256SUMS line for `vouched`.
fn release(dir: &Path, vouched: &[u8], served: &[u8]) -> Result<(), Box<dyn Error>> {
    let name = format!("yi-9.9.9-{}.tar.gz", triple()?);
    std::fs::write(dir.join(&name), served)?;
    let sum = yi_runtime::tarball::sha256_hex(vouched);
    std::fs::write(dir.join("SHA256SUMS"), format!("{sum}  {name}\n"))?;
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the contract is the script as bash runs it"
)]
fn install(scratch: &Scratch, base: &Path) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new("bash")
        .arg(SCRIPT)
        .env("HOME", scratch.join("home"))
        .env("YI_RELEASE_BASE", format!("file://{}", base.display()))
        .env_remove("YI_PREFIX")
        .env_remove("YI_VERSION")
        .current_dir(&**scratch)
        .output()?)
}

#[test]
fn a_vouched_archive_installs_the_binary_its_runtime_and_receipt() -> TestResult {
    let scratch = Scratch::new("yi-install-ok")?;
    let base = scratch.join("release");
    std::fs::create_dir_all(&base)?;
    let name = format!("yi-9.9.9-{}", triple()?);
    let good = archive(&scratch, &name, "yi 9.9.9")?;
    release(&base, &good, &good)?;

    let out = install(&scratch, &base)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let home = scratch.join("home");
    assert!(stdout.contains("installed yi 9.9.9 at"), "{stdout}");
    assert!(home.join(".yi/python/yi_runtime/__init__.py").is_file());
    assert!(home.join(".yi/skills/yi/pr-review/SKILL.md").is_file());
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join(".yi/install.json"))?)?;
    assert_eq!(receipt["version"], "9.9.9");
    assert_eq!(receipt["target"], triple()?.as_str());
    Ok(())
}

#[test]
fn an_archive_whose_sha256_differs_from_sha256sums_installs_nothing() -> TestResult {
    let scratch = Scratch::new("yi-install-tampered")?;
    let base = scratch.join("release");
    std::fs::create_dir_all(&base)?;
    let name = format!("yi-9.9.9-{}", triple()?);
    let good = archive(&scratch, &name, "yi 9.9.9")?;
    let swapped = archive(&scratch, &name, "yi 9.9.9 swapped")?;
    release(&base, &good, &swapped)?;

    let out = install(&scratch, &base)?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("but SHA256SUMS says") && stderr.contains("nothing installed"),
        "{stderr}"
    );
    let home = scratch.join("home");
    assert!(!home.join(".local/bin/yi").exists());
    assert!(!home.join(".yi").exists());
    Ok(())
}
