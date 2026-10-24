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
        let (_scratch, swapped) = (scratch, None);
        Ok((
            Self {
                _scratch,
                store,
                d,
                swapped,
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

/// A racing thread landed this swap a few times in 20,000 tries; here it lands every time.
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

/// The race found in review landed once in 100,000 tries: the delete must stay in the
/// directory judged.
#[test]
fn a_parent_swapped_after_the_judgement_deletes_in_the_judged_dir() -> TestResult {
    let (mut swap, gate) = Swap::new("yi-gate-unlink")?;
    let target = swap.d.join("id_rsa");
    let removed = gate.remove_after(&target, &[], || swap.swap());
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
