//! `ReadGate` judges the file it opened (#890): `d` turns into a link to the key store at the
//! one moment a racing process could use, between the gate's judgement and its act, every run.
#![cfg(unix)]

use std::error::Error;
use std::io;
use std::path::PathBuf;

use yi_permission::{CatastrophicContext, ReadGate};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const KEY: &str = "FAKE KEY MARKER\n";

/// `workspace/d` holds an ordinary `id_rsa` beside a home whose `.ssh` holds the key.
struct Swap {
    _scratch: Scratch,
    store: PathBuf,
    d: PathBuf,
    swapped: Option<io::Result<()>>,
}

impl Swap {
    fn new(name: &str) -> Result<(Self, ReadGate), Box<dyn Error>> {
        let scratch = Scratch::new(name)?;
        let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
        let (store, d) = (home.join(".ssh"), workspace.join("d"));
        std::fs::create_dir_all(&store)?;
        std::fs::create_dir_all(&d)?;
        std::fs::write(store.join("id_rsa"), KEY)?;
        std::fs::write(d.join("id_rsa"), "ordinary\n")?;
        let gate = ReadGate::new(&CatastrophicContext {
            home_dir: Some(home),
            working_dir: Some(workspace),
            workspace_git: Vec::new(),
            host_owned: Vec::new(),
        });
        Ok((
            Self {
                _scratch: scratch,
                store,
                d,
                swapped: None,
            },
            gate,
        ))
    }

    fn swap(&mut self) {
        let moved = std::fs::rename(&self.d, self.d.with_file_name("dir.tmp"));
        let linked = moved.and_then(|()| std::os::unix::fs::symlink(&self.store, &self.d));
        self.swapped = Some(linked);
    }

    /// The key's text once the swap ran; a call that never reached its hold proved nothing.
    fn key(&mut self) -> Result<String, Box<dyn Error>> {
        self.swapped
            .take()
            .ok_or("the call never reached the swap")??;
        Ok(std::fs::read_to_string(self.store.join("id_rsa")).unwrap_or_default())
    }
}

#[test]
fn a_parent_swapped_after_the_judgement_reads_no_key() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-read")?;
    let target = swap.d.join("id_rsa");
    let opened = gate.open_after(&target, &[], &mut || swap.swap());
    let read = opened.and_then(io::read_to_string);
    swap.key()?;
    assert!(
        read.is_err(),
        "the open followed the swapped parent: {read:?}"
    );
    Ok(())
}

#[test]
fn a_parent_swapped_after_the_judgement_writes_no_key() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-write")?;
    let target = swap.d.join("id_rsa");
    let opened = gate.open_write_after(&target, &[], &mut || swap.swap());
    let wrote = opened.and_then(|mut file| io::Write::write_all(&mut file, b"overwritten\n"));
    assert_eq!(swap.key()?, KEY, "the write reached the key");
    assert!(wrote.is_err(), "the write opened the swapped parent");
    Ok(())
}

/// A missing file is judged again before its create, so the swap lands at that second
/// judgement: on Linux the directory is held open, on macOS no link is on the path.
#[test]
fn a_parent_swapped_after_the_judgement_creates_nothing_in_the_store() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-create")?;
    let (target, mut judged) = (swap.d.join("fresh"), 0);
    let created = gate.open_write_after(&target, &[], &mut || {
        judged += 1;
        if judged == 2 {
            swap.swap();
        }
    });
    swap.key()?;
    assert!(
        !swap.store.join("fresh").exists(),
        "the create landed in the store"
    );
    assert!(created.is_err(), "the create opened the swapped parent");
    Ok(())
}

/// A swap before the parent is opened is refused: the parent opened is the one judged.
#[test]
fn a_parent_swapped_before_it_is_opened_deletes_no_key() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-unlink-open")?;
    let (target, mut held) = (swap.d.join("id_rsa"), 0);
    let removed = gate.remove_after(&target, &[], &mut || {
        held += 1;
        if held == 1 {
            swap.swap();
        }
    });
    assert_eq!(swap.key()?, KEY, "the remove deleted the key");
    assert!(removed.is_err(), "the remove opened the swapped parent");
    Ok(())
}

/// A swap once the parent is held open deletes in the directory judged, not behind the link.
#[test]
fn a_parent_swapped_after_the_judgement_deletes_in_the_judged_dir() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-unlink")?;
    let (target, mut held) = (swap.d.join("id_rsa"), 0);
    let removed = gate.remove_after(&target, &[], &mut || {
        held += 1;
        if held == 2 {
            swap.swap();
        }
    });
    assert_eq!(
        swap.key()?,
        KEY,
        "the remove deleted the key behind the swapped parent"
    );
    assert!(removed.is_ok(), "{removed:?}");
    let left = swap.d.with_file_name("dir.tmp/id_rsa").exists();
    assert!(!left, "the remove left the file in the directory it judged");
    Ok(())
}

#[test]
fn a_hard_link_to_a_key_file_is_refused() -> TestResult {
    let scratch = Scratch::new("yi-gate-hardlink")?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    let key = home.join(".ssh/id_rsa");
    let alias = workspace.join("key-copy");
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(&key, KEY)?;
    std::fs::hard_link(&key, &alias)?;
    let gate = ReadGate::new(&CatastrophicContext {
        home_dir: Some(home),
        working_dir: Some(workspace),
        workspace_git: Vec::new(),
        host_owned: Vec::new(),
    });

    let denied = gate.denies(&alias);
    let opened = gate.open(&alias, &[]).and_then(io::read_to_string);
    let entry_metadata = std::fs::symlink_metadata(&alias)?;
    let entry_denied = gate.denies_entry(Some(&entry_metadata));
    let write_denied = match gate.open_write(&alias, &[]) {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, b"OVERWRITTEN\n")?;
            false
        }
        Err(_) => true,
    };
    let key_unchanged = std::fs::read_to_string(&key)? == KEY;

    assert!(denied, "ReadGate::denies allowed the hard-link alias");
    assert!(opened.is_err(), "ReadGate::open opened the hard-link alias");
    assert!(
        write_denied,
        "ReadGate::open_write opened the hard-link alias"
    );
    assert!(
        entry_denied,
        "ReadGate::denies_entry allowed the hard-link alias"
    );
    assert!(key_unchanged, "the hard-link alias changed the key");
    Ok(())
}

#[test]
fn a_hard_link_swapped_in_after_the_judgement_writes_no_key() -> TestResult {
    let scratch = Scratch::new("yi-gate-hardlink-swap")?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    let (key, target) = (home.join(".ssh/id_rsa"), workspace.join("copy"));
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(&key, KEY)?;
    std::fs::write(&target, "ordinary\n")?;
    let gate = ReadGate::new(&CatastrophicContext {
        home_dir: Some(home),
        working_dir: Some(workspace),
        workspace_git: Vec::new(),
        host_owned: Vec::new(),
    });
    let mut swapped = None;
    let opened = gate.open_write_after(&target, &[], &mut || {
        swapped =
            Some(std::fs::remove_file(&target).and_then(|()| std::fs::hard_link(&key, &target)));
    });
    swapped.ok_or("the write never reached its open")??;
    let wrote = opened.and_then(|mut file| std::io::Write::write_all(&mut file, b"OVERWRITTEN\n"));
    assert!(wrote.is_err(), "the open wrote through a swapped hard link");
    assert_eq!(
        std::fs::read_to_string(&key)?,
        KEY,
        "the hard link changed the key"
    );
    Ok(())
}

#[test]
fn a_hard_link_to_a_key_beneath_a_store_descendant_symlink_is_refused() -> TestResult {
    let scratch = Scratch::new("yi-gate-symlink-hardlink")?;
    let (home, workspace, external_keys) = (
        scratch.join("home"),
        scratch.join("workspace"),
        scratch.join("external/keys"),
    );
    let key = external_keys.join("id_rsa");
    let alias = workspace.join("key-copy");
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(&external_keys)?;
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(&key, KEY)?;
    std::os::unix::fs::symlink(&external_keys, home.join(".ssh/keys"))?;
    std::fs::hard_link(&key, &alias)?;
    let gate = ReadGate::new(&CatastrophicContext {
        home_dir: Some(home),
        working_dir: Some(workspace),
        workspace_git: Vec::new(),
        host_owned: Vec::new(),
    });

    assert!(
        gate.denies(&alias),
        "ReadGate::denies allowed the hard-link alias"
    );
    assert!(
        gate.open(&alias, &[]).and_then(io::read_to_string).is_err(),
        "ReadGate::open opened the hard-link alias"
    );
    Ok(())
}
