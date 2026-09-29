#![cfg(unix)]
//! D316: a kept spill is private, and a name already taken is never followed or overwritten.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use yi_tools::spill::Spill;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn kept(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let mut spill = Spill::new(Some(dir));
    spill.write(bytes);
    let note = spill.keep().ok_or("not kept")?;
    let path = note
        .strip_prefix("[full output: ")
        .and_then(|rest| rest.strip_suffix(']'));
    path.map(PathBuf::from).ok_or(note)
}

#[test]
fn a_kept_spill_is_private_and_an_unkept_one_leaves_nothing() -> Fallible {
    let root = Scratch::new("yi-spill-mode")?;
    let dir = root.join("tool-output");
    let path = kept(&dir, b"secret")?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
    let mut dropped = Spill::new(Some(&dir));
    dropped.write(&[b'x'; 64 * 1024 + 1]);
    drop(dropped);
    assert_eq!(
        fs::read_dir(&dir)?.count(),
        1,
        "the .part file outlived its spill"
    );
    Ok(())
}

#[test]
fn a_taken_name_is_neither_followed_nor_overwritten() -> Fallible {
    let root = Scratch::new("yi-spill-taken")?;
    let dir = root.join("tool-output");
    fs::create_dir_all(&dir)?;
    let name = format!("{:032x}.txt", xxhash_rust::xxh3::xxh3_128(b"output"));
    let victim = root.join("victim");
    fs::write(&victim, "victim")?;
    std::os::unix::fs::symlink(&victim, dir.join(&name))?;
    let first = kept(&dir, b"output")?;
    assert_eq!(
        fs::read_to_string(&victim)?,
        "victim",
        "the link was followed"
    );
    assert!(fs::symlink_metadata(dir.join(&name))?.is_symlink());
    assert_eq!(fs::read(&first)?, b"output");
    // Two different outputs on one name: the planted one stays, the new one gets its own.
    fs::remove_file(dir.join(&name))?;
    fs::write(dir.join(&name), "other")?;
    let second = kept(&dir, b"output")?;
    assert_eq!(fs::read_to_string(dir.join(&name))?, "other");
    assert_eq!(fs::read(&second)?, b"output");
    fs::write(dir.join(&name), "output")?;
    assert_eq!(
        kept(&dir, b"output")?,
        dir.join(&name),
        "equal bytes reuse the name"
    );
    Ok(())
}
