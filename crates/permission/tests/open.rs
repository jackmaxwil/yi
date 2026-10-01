//! `ReadGate` judges the file it opened (#890): a thread flips `d` between a directory and a
//! link to the key store while the gate resolves, judges and opens `d/id_rsa` in a tight loop.
#![cfg(unix)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use yi_permission::{CatastrophicContext, ReadGate};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

type TestResult = Result<(), Box<dyn Error>>;

const KEY: &str = "FAKE KEY MARKER\n";

/// One call against the flipping path: the gate, the target, the walls and the key; true when
/// the call reached the key.
type Try = dyn Fn(&ReadGate, &Path, &[PathBuf], &Path) -> Result<bool, Box<dyn Error>>;

/// Calls `try_once` against `d/id_rsa` while `d` flips, up to 20,000 times or 5 s, and counts
/// the calls `try_once` says reached the key.
fn flipping(name: &str, try_once: &Try) -> Result<(usize, usize), Box<dyn Error>> {
    let scratch = scratch::Scratch::new(name)?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(workspace.join("d"))?;
    let key = home.join(".ssh/id_rsa");
    std::fs::write(&key, KEY)?;
    let gate = ReadGate::new(&CatastrophicContext {
        home_dir: Some(home.clone()),
        working_dir: Some(workspace.clone()),
        workspace_git: Vec::new(),
        host_owned: Vec::new(),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let flipper = {
        let (stop, store) = (Arc::clone(&stop), home.join(".ssh"));
        let at = |name: &str| -> PathBuf { workspace.join(name) };
        let (d, stash, file) = (at("d"), at("dir.tmp"), at("d/id_rsa"));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::write(&file, "ordinary\n");
                let _ = std::fs::rename(&d, &stash);
                let _ = std::os::unix::fs::symlink(&store, &d);
                let _ = std::fs::remove_file(&d);
                let _ = std::fs::rename(&stash, &d);
            }
        })
    };
    let target = workspace.join("d/id_rsa");
    // A long wall list is judged by a stat each, which widens the gap between the gate's
    // resolve and its open the way a slow disk would.
    let walls: Vec<PathBuf> = (0..400)
        .map(|n| workspace.join(format!("wall{n}")))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut tries, mut leaks) = (0_usize, 0_usize);
    while tries < 20_000 && Instant::now() < deadline {
        tries += 1;
        if try_once(&gate, &target, &walls, &key)? {
            leaks += 1;
            std::fs::write(&key, KEY)?;
        }
    }
    stop.store(true, Ordering::Relaxed);
    flipper.join().map_err(|_| "the flipper panicked")?;
    Ok((tries, leaks))
}

#[test]
fn a_parent_swapped_under_the_gate_reads_no_key() -> TestResult {
    let (tries, leaks) = flipping("yi-gate-read", &|gate, target, walls, _| {
        let opened = gate.open(target, walls).and_then(std::io::read_to_string);
        Ok(opened.is_ok_and(|text| text.contains("MARKER")))
    })?;
    assert_eq!(leaks, 0, "{leaks} of {tries} opens read the key");
    Ok(())
}

#[test]
fn a_parent_swapped_under_the_gate_writes_no_key() -> TestResult {
    let (tries, leaks) = flipping("yi-gate-write", &|gate, target, walls, key| {
        if let Ok(mut file) = gate.open_write(target, walls) {
            std::io::Write::write_all(&mut file, b"overwritten\n")?;
        }
        Ok(std::fs::read_to_string(key)? != KEY)
    })?;
    assert_eq!(leaks, 0, "{leaks} of {tries} writes changed the key");
    Ok(())
}

/// A missing file is created only in the directory judged: on Linux through that directory held
/// open, on macOS with no link on the path, so nothing new appears in the store.
#[test]
fn a_parent_swapped_under_the_gate_creates_nothing_in_the_store() -> TestResult {
    let (tries, leaks) = flipping("yi-gate-create", &|gate, target, walls, key| {
        let fresh = target.with_file_name("fresh");
        let _ = gate.open_write(&fresh, walls);
        let stray = key.with_file_name("fresh");
        let planted = stray.exists();
        let _ = std::fs::remove_file(stray);
        let _ = std::fs::remove_file(fresh);
        Ok(planted)
    })?;
    assert_eq!(leaks, 0, "{leaks} of {tries} creates landed in the store");
    Ok(())
}

/// Linux unlinks inside the directory the gate held open; macOS has no std way to, so its
/// delete keeps the gap D339 names and is not pinned here.
#[cfg(target_os = "linux")]
#[test]
fn a_parent_swapped_under_the_gate_deletes_no_key() -> TestResult {
    let (tries, leaks) = flipping("yi-gate-remove", &|gate, target, walls, key| {
        let _ = gate.remove(target, walls);
        Ok(!key.exists())
    })?;
    assert_eq!(leaks, 0, "{leaks} of {tries} removes deleted the key");
    Ok(())
}
