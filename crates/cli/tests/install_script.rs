//! The release scripts as a release runs them: `scripts/package.sh` writes the archive and its
//! `.sha256`, `just publish` folds those into SHA256SUMS, and `scripts/install.sh`, with no release
//! tree beside it, installs only what SHA256SUMS vouches for.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const UNSIGNED: [(&str, &str); 1] = [("YI_RELEASE_UNSIGNED", "1")];

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

/// Runs package.sh on a stand-in binary that prints `says`; each test owns its `version`, since
/// package.sh writes under the repository's target/package.
#[expect(
    clippy::disallowed_methods,
    reason = "the producer under test is package.sh as the release runs it"
)]
fn package(
    scratch: &Scratch,
    version: &str,
    says: &str,
    env: &[(&str, &str)],
) -> Result<Output, Box<dyn Error>> {
    let binary = scratch.join("yi");
    std::fs::write(&binary, format!("#!/bin/sh\necho '{says}'\n"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    let mut run = Command::new("bash");
    run.arg(format!("{ROOT}/scripts/package.sh"))
        .args([version, triple()?.as_str()])
        .arg(&binary)
        .env_remove("YI_RELEASE_UNSIGNED")
        .env("YI_SIGNING_KEY", scratch.join("no-such-key"));
    for (key, value) in env {
        run.env(key, value);
    }
    Ok(run.output()?)
}

fn packaged(version: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(PathBuf::from(format!(
        "{ROOT}/target/package/yi-{version}-{}.tar.gz",
        triple()?
    )))
}

fn beside(archive: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", archive.display()))
}

/// Removes what package.sh left under target/package for `version`.
fn unpackage(version: &str) -> Result<(), Box<dyn Error>> {
    let archive = packaged(version)?;
    let _ = std::fs::remove_dir_all(format!("{ROOT}/target/package/yi-{version}-{}", triple()?));
    for suffix in ["", ".sha256", ".sig"] {
        let _ = std::fs::remove_file(beside(&archive, suffix));
    }
    Ok(())
}

/// A release directory holding `served` under the archive's name, beside the SHA256SUMS that
/// `just publish` folds from the `.sha256` package.sh wrote for the archive it vouches for.
fn release(dir: &Path, version: &str, served: &[u8]) -> Result<(), Box<dyn Error>> {
    let archive = packaged(version)?;
    let name = archive.file_name().ok_or("the archive path has no name")?;
    std::fs::create_dir_all(dir)?;
    std::fs::copy(beside(&archive, ".sha256"), dir.join("SHA256SUMS"))?;
    std::fs::write(dir.join(name), served)?;
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the contract is the script as bash runs it"
)]
fn install(scratch: &Scratch, base: &Path, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new("bash")
        .arg(format!("{ROOT}/scripts/install.sh"))
        .args(args)
        .env("HOME", scratch.join("home"))
        .env("YI_RELEASE_BASE", format!("file://{}", base.display()))
        .env_remove("YI_PREFIX")
        .env_remove("YI_VERSION")
        .current_dir(&**scratch)
        .output()?)
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_vouched_archive_installs_the_binary_its_runtime_and_receipt() -> TestResult {
    let scratch = Scratch::new("yi-install-ok")?;
    let packed = package(&scratch, "9.9.1", "yi 9.9.1", &UNSIGNED)?;
    assert!(packed.status.success(), "{}", text(&packed));
    release(
        &scratch.join("release"),
        "9.9.1",
        &std::fs::read(packaged("9.9.1")?)?,
    )?;
    unpackage("9.9.1")?;

    let out = install(&scratch, &scratch.join("release"), &[])?;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("installed yi 9.9.1 at"), "{said}");
    assert!(
        said.contains("this release is unsigned; integrity is the sha256 check only"),
        "{said}"
    );
    let home = scratch.join("home");
    assert!(home.join(".yi/python/yi_runtime").is_dir());
    assert!(home.join(".yi/skills/yi").is_dir());
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join(".yi/install.json"))?)?;
    assert_eq!(receipt["version"], "9.9.1");
    assert_eq!(receipt["target"], triple()?.as_str());
    Ok(())
}

#[test]
fn an_archive_whose_sha256_differs_from_sha256sums_installs_nothing() -> TestResult {
    let scratch = Scratch::new("yi-install-tampered")?;
    assert!(
        package(&scratch, "9.9.2", "yi 9.9.2 swapped", &UNSIGNED)?
            .status
            .success()
    );
    let swapped = std::fs::read(packaged("9.9.2")?)?;
    assert!(
        package(&scratch, "9.9.2", "yi 9.9.2", &UNSIGNED)?
            .status
            .success()
    );
    release(&scratch.join("release"), "9.9.2", &swapped)?;
    unpackage("9.9.2")?;

    let out = install(&scratch, &scratch.join("release"), &[])?;
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(
        said.contains("but SHA256SUMS says") && said.contains("nothing installed"),
        "{said}"
    );
    assert!(!scratch.join("home").exists());
    Ok(())
}

#[test]
fn a_dry_run_names_the_archive_and_writes_nothing() -> TestResult {
    let scratch = Scratch::new("yi-install-dry")?;
    let packed = package(&scratch, "9.9.3", "yi 9.9.3", &UNSIGNED)?;
    assert!(packed.status.success(), "{}", text(&packed));
    release(
        &scratch.join("release"),
        "9.9.3",
        &std::fs::read(packaged("9.9.3")?)?,
    )?;
    unpackage("9.9.3")?;

    let out = install(&scratch, &scratch.join("release"), &["--dry-run"])?;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("would download file://") && said.contains("yi-9.9.3-"),
        "{said}"
    );
    assert!(!scratch.join("home").exists());
    Ok(())
}

#[test]
fn package_signs_by_default_and_leaves_no_archive_it_could_not_sign() -> TestResult {
    let scratch = Scratch::new("yi-package-signs")?;
    let out = package(&scratch, "9.9.4", "yi 9.9.4", &[])?;
    let left = packaged("9.9.4")?.exists();
    unpackage("9.9.4")?;
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no signing key"), "{}", text(&out));
    assert!(!left, "an unsigned archive was left to publish");

    let out = package(&scratch, "9.9.5", "yi 9.9.5", &UNSIGNED)?;
    let archive = packaged("9.9.5")?;
    let (kept, signed) = (archive.exists(), beside(&archive, ".sig").exists());
    unpackage("9.9.5")?;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("UNSIGNED:") && said.contains("yi update refuses it"),
        "{said}"
    );
    assert!(kept && !signed);
    Ok(())
}
