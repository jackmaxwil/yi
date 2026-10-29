//! The release scripts as a release runs them: `scripts/package.sh` writes and signs the archive,
//! `just publish` folds the `.sha256` files into SHA256SUMS, and `scripts/install.sh`, with no
//! release tree beside it, installs only what SHA256SUMS vouches for and the signature confirms.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

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

/// A throwaway release key under `dir` and the one-line signers file naming it `who`.
#[expect(
    clippy::disallowed_methods,
    reason = "release keys are ssh-keygen keys, as scripts/sign.sh makes them"
)]
fn key(dir: &Path, who: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    std::fs::create_dir_all(dir)?;
    let key = dir.join("key");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", who, "-f"])
        .arg(&key)
        .status()?;
    if !made.success() {
        return Err("ssh-keygen could not make a key".into());
    }
    let public = std::fs::read_to_string(dir.join("key.pub"))?;
    let signers = dir.join("allowed_signers");
    std::fs::write(&signers, format!("{who} {}\n", public.trim()))?;
    Ok((key, signers))
}

/// Runs package.sh, signing with `key`, on a stand-in binary that prints `says`; each test owns
/// its `version`, since package.sh writes under the repository's target/package.
#[expect(
    clippy::disallowed_methods,
    reason = "the producer under test is package.sh as the release runs it"
)]
fn package(
    scratch: &Scratch,
    version: &str,
    says: &str,
    key: &(PathBuf, PathBuf),
) -> Result<Output, Box<dyn Error>> {
    let binary = scratch.join("yi");
    std::fs::write(&binary, format!("#!/bin/sh\necho '{says}'\n"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    Ok(Command::new("bash")
        .arg(format!("{ROOT}/scripts/package.sh"))
        .args([version, triple()?.as_str()])
        .arg(&binary)
        .env("YI_SIGNING_KEY", &key.0)
        .env("YI_ALLOWED_SIGNERS", &key.1)
        .output()?)
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

/// Takes what package.sh left under target/package for `version`: the archive, its digest line
/// and its signature, each None when package.sh did not write it.
fn unpackage(version: &str) -> Result<[Option<Vec<u8>>; 3], Box<dyn Error>> {
    let archive = packaged(version)?;
    let _ = std::fs::remove_dir_all(format!("{ROOT}/target/package/yi-{version}-{}", triple()?));
    let mut taken = [None, None, None];
    for (slot, suffix) in taken.iter_mut().zip(["", ".sha256", ".sig"]) {
        *slot = std::fs::read(beside(&archive, suffix)).ok();
        let _ = std::fs::remove_file(beside(&archive, suffix));
    }
    Ok(taken)
}

/// A release directory as `just publish` uploads it: `served` under the archive's name, the
/// SHA256SUMS folded from package.sh's `.sha256` under its build line, and any `.sig`.
fn release(
    dir: &Path,
    version: &str,
    served: &[u8],
    digest: &[u8],
    sig: Option<&[u8]>,
) -> Result<(), Box<dyn Error>> {
    let archive = packaged(version)?;
    let name = archive.file_name().ok_or("the archive path has no name")?;
    std::fs::create_dir_all(dir)?;
    let head = format!("# yi {version} forge build of commit {}\n", "0".repeat(40));
    std::fs::write(dir.join("SHA256SUMS"), [head.as_bytes(), digest].concat())?;
    std::fs::write(dir.join(name), served)?;
    if let Some(sig) = sig {
        std::fs::write(beside(&dir.join(name), ".sig"), sig)?;
    }
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the contract is the script as bash runs it"
)]
fn install(scratch: &Scratch, base: &Path, signers: &Path) -> Command {
    let mut bash = Command::new("bash");
    bash.arg(format!("{ROOT}/scripts/install.sh"))
        .env("HOME", scratch.join("home"))
        .env("YI_RELEASE_BASE", format!("file://{}", base.display()))
        .env("YI_ALLOWED_SIGNERS", signers)
        .env_remove("YI_PREFIX")
        .env_remove("YI_VERSION")
        .current_dir(&**scratch);
    bash
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A signed release of `version` under `scratch/release`, packaged by package.sh with `key`.
fn signed_release(
    scratch: &Scratch,
    version: &str,
    key: &(PathBuf, PathBuf),
) -> Result<(), Box<dyn Error>> {
    let packed = package(scratch, version, &format!("yi {version}"), key)?;
    let [archive, digest, sig] = unpackage(version)?;
    let (Some(archive), Some(digest)) = (archive, digest) else {
        return Err(format!("package.sh wrote no archive: {}", text(&packed)).into());
    };
    release(
        &scratch.join("release"),
        version,
        &archive,
        &digest,
        sig.as_deref(),
    )
}

#[test]
fn a_vouched_and_signed_archive_installs_the_binary_its_runtime_and_receipt() -> TestResult {
    let scratch = Scratch::new("yi-install-ok")?;
    let release_key = key(&scratch.join("keys"), "test-release")?;
    signed_release(&scratch, "9.9.1", &release_key)?;

    let out = install(&scratch, &scratch.join("release"), &release_key.1).output()?;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("is signed by test-release"), "{said}");
    assert!(said.contains("installed yi 9.9.1 at"), "{said}");
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
    let release_key = key(&scratch.join("keys"), "test-release")?;
    package(&scratch, "9.9.2", "yi 9.9.2 swapped", &release_key)?;
    let [swapped, ..] = unpackage("9.9.2")?;
    package(&scratch, "9.9.2", "yi 9.9.2", &release_key)?;
    let [_, digest, sig] = unpackage("9.9.2")?;
    let (Some(swapped), Some(digest)) = (swapped, digest) else {
        return Err("package.sh wrote no archive".into());
    };
    release(
        &scratch.join("release"),
        "9.9.2",
        &swapped,
        &digest,
        sig.as_deref(),
    )?;

    let out = install(&scratch, &scratch.join("release"), &release_key.1).output()?;
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
fn an_archive_signed_by_another_key_installs_nothing() -> TestResult {
    let scratch = Scratch::new("yi-install-forged")?;
    let forger = key(&scratch.join("forger"), "test-release")?;
    let release_key = key(&scratch.join("keys"), "test-release")?;
    signed_release(&scratch, "9.9.6", &forger)?;

    let out = install(&scratch, &scratch.join("release"), &release_key.1).output()?;
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(
        said.contains("is not test-release's signature") && said.contains("nothing installed"),
        "{said}"
    );
    assert!(!scratch.join("home").exists());
    Ok(())
}

#[test]
fn a_release_without_a_signature_says_the_sha256_is_its_only_check() -> TestResult {
    let scratch = Scratch::new("yi-install-nosig")?;
    let release_key = key(&scratch.join("keys"), "test-release")?;
    package(&scratch, "9.9.7", "yi 9.9.7", &release_key)?;
    let [archive, digest, _] = unpackage("9.9.7")?;
    let (Some(archive), Some(digest)) = (archive, digest) else {
        return Err("package.sh wrote no archive".into());
    };
    release(&scratch.join("release"), "9.9.7", &archive, &digest, None)?;

    let out = install(&scratch, &scratch.join("release"), &release_key.1).output()?;
    let said = text(&out);
    let notice = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    assert!(
        notice.contains("this release has no signature; integrity is the sha256 check only"),
        "{said}"
    );
    assert!(said.contains("installed yi 9.9.7 at"), "{said}");
    Ok(())
}

#[test]
fn a_signature_fetch_that_fails_but_not_for_absence_installs_nothing() -> TestResult {
    let scratch = Scratch::new("yi-install-sigfetch")?;
    let release_key = key(&scratch.join("keys"), "test-release")?;
    signed_release(&scratch, "9.9.3", &release_key)?;
    // curl's 6 is a host that did not resolve: the .sig may well exist on the release.
    let (shim, path) = (scratch.join("bin"), std::env::var("PATH")?);
    std::fs::create_dir_all(&shim)?;
    let curl = format!(
        "#!/bin/sh\ncase \"$*\" in *.sig*) exit 6 ;; esac\nPATH='{path}' exec curl \"$@\"\n"
    );
    std::fs::write(shim.join("curl"), curl)?;
    std::fs::set_permissions(shim.join("curl"), std::fs::Permissions::from_mode(0o755))?;

    let out = install(&scratch, &scratch.join("release"), &release_key.1)
        .env("PATH", format!("{}:{path}", shim.display()))
        .output()?;
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(
        said.contains("fetching yi-9.9.3-") && said.contains("(curl exit 6); nothing installed"),
        "{said}"
    );
    assert!(!said.contains("has no signature"), "{said}");
    assert!(!scratch.join("home/.local/bin/yi").exists());
    Ok(())
}

#[test]
fn package_signs_and_leaves_no_archive_it_could_not_sign() -> TestResult {
    let scratch = Scratch::new("yi-package-signs")?;
    let missing = (scratch.join("no-such-key"), scratch.join("no-such-signers"));
    let out = package(&scratch, "9.9.4", "yi 9.9.4", &missing)?;
    let [left, ..] = unpackage("9.9.4")?;
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no signing key"), "{}", text(&out));
    assert!(left.is_none(), "an unsigned archive was left to publish");

    let release_key = key(&scratch.join("keys"), "test-release")?;
    let out = package(&scratch, "9.9.5", "yi 9.9.5", &release_key)?;
    let [archive, _, sig] = unpackage("9.9.5")?;
    assert!(out.status.success(), "{}", text(&out));
    assert!(archive.is_some() && sig.is_some(), "{}", text(&out));
    Ok(())
}

#[test]
fn the_installer_trusts_the_one_key_yi_update_compiles_in() -> TestResult {
    let scratch = Scratch::new("yi-install-signer")?;
    let signers = std::fs::read_to_string(format!("{ROOT}/allowed_signers"))?;
    let out = install(&scratch, &scratch.join("release"), Path::new(""))
        .arg("--help")
        .output()?;
    assert!(out.status.success(), "{}", text(&out));
    let trusted = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        signers.lines().count(),
        1,
        "allowed_signers holds more than one signer"
    );
    assert!(
        trusted.contains(signers.trim()),
        "--help trusts:\n{trusted}\nallowed_signers:\n{signers}"
    );
    Ok(())
}
