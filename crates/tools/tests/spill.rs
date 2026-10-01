#![cfg(unix)]
//! D316: a kept spill is private, and a name already taken is never followed or overwritten.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../src/spill.rs"]
mod spill;
use spill::Spill;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type Fallible = Result<(), Box<dyn std::error::Error>>;

/// Counts live heap bytes, so a test can bound what one call holds at once.
struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` with the caller's arguments unchanged; the counters
// are plain atomics and never allocate.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, passed through as is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` came from `alloc` above with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn kept(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let mut spill = Spill::new(Some(dir));
    spill.write(bytes);
    let note = spill.keep().ok_or("not kept")?;
    let path = note
        .strip_prefix("[full output: ")
        .and_then(|rest| rest.strip_suffix(']'));
    path.map(PathBuf::from).ok_or(note)
}

fn mode(path: &Path) -> std::io::Result<u32> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777)
}

#[test]
fn a_kept_spill_is_private_and_an_unkept_one_leaves_nothing() -> Fallible {
    let root = Scratch::new("yi-spill-mode")?;
    let dir = root.join("tool-output");
    let path = kept(&dir, b"secret")?;
    assert_eq!(mode(&path)?, 0o600);
    assert_eq!(mode(&dir)?, 0o700);
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

/// #881: a linked spill root is refused, so neither a spill nor the sweep reaches its target.
#[test]
fn a_linked_spill_root_is_refused_and_its_target_left_alone() -> Fallible {
    let root = Scratch::new("yi-spill-link")?;
    let target = root.join("target");
    fs::create_dir_all(target.join("s1"))?;
    let old = target.join("s1/old.txt");
    fs::write(&old, "a week old")?;
    let file = fs::OpenOptions::new().write(true).open(&old)?;
    file.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
    std::os::unix::fs::symlink(&target, root.join("spills"))?;
    assert_eq!(
        kept(&root.join("spills/s1"), &[b'x'; 70_000]),
        Err("not kept".to_owned())
    );
    let mut left: Vec<_> = fs::read_dir(target.join("s1"))?
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["old.txt"],
        "the spill or the sweep went through the link"
    );
    Ok(())
}

/// #881: a root and session dir left 0755 by the old tee are made 0700 again.
#[test]
fn an_open_spill_root_is_made_private() -> Fallible {
    let root = Scratch::new("yi-spill-chmod")?;
    let spills = root.join("spills");
    fs::create_dir_all(spills.join("s1"))?;
    for dir in [&spills, &spills.join("s1")] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    }
    kept(&spills.join("s1"), b"output")?;
    assert_eq!((mode(&spills)?, mode(&spills.join("s1"))?), (0o700, 0o700));
    Ok(())
}

/// #881: the `.part` name is not `.<pid>-<n>`, so a planted one cannot turn spilling off.
#[test]
fn a_planted_part_name_leaves_spilling_on() -> Fallible {
    let root = Scratch::new("yi-spill-part")?;
    let dir = root.join("spills/s1");
    fs::create_dir_all(&dir)?;
    for n in 0..8 {
        fs::write(dir.join(format!(".{}-{n}.part", std::process::id())), "")?;
    }
    kept(&dir, &[b'x'; 70_000])?;
    Ok(())
}

/// #881: a repeated output meets its own name and is compared in chunks, never read whole.
#[test]
fn a_taken_name_is_compared_in_bounded_memory() -> Fallible {
    const SIZE: usize = 32 << 20;
    let root = Scratch::new("yi-spill-memory")?;
    let dir = root.join("spills/s1");
    let chunk = vec![b'y'; 1 << 20];
    let write = |spill: &mut Spill| (0..SIZE / chunk.len()).for_each(|_| spill.write(&chunk));
    let mut first = Spill::new(Some(&dir));
    write(&mut first);
    first.keep().ok_or("first not kept")?;
    let mut again = Spill::new(Some(&dir));
    write(&mut again);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let base = LIVE.load(Ordering::Relaxed);
    let note = again.keep().ok_or("second not kept")?;
    let held = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    assert!(held < 4 << 20, "the compare held {held} bytes at once");
    assert_eq!(
        fs::read_dir(&dir)?.count(),
        1,
        "equal bytes reuse the name: {note}"
    );
    Ok(())
}

/// D316: a keep sweeps every session's dir under the root, not only its own. The sweep runs
/// once an hour per process, so this holds under nextest, which gives each test its own.
#[test]
fn a_keep_sweeps_other_sessions_old_spills() -> Fallible {
    let root = Scratch::new("yi-spill-sweep-all")?;
    let old = root.join("spills/s2/old.txt");
    fs::create_dir_all(root.join("spills/s2"))?;
    fs::write(&old, "a week old")?;
    let file = fs::OpenOptions::new().write(true).open(&old)?;
    file.set_modified(std::time::SystemTime::UNIX_EPOCH)?;
    kept(&root.join("spills/s1"), b"output")?;
    assert!(
        !old.exists(),
        "another session's old spill outlived the sweep"
    );
    Ok(())
}
