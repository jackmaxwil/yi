#![cfg(unix)]
//! D316: a kept spill is private, and a name already taken is never followed or overwritten.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../src/redact.rs"]
mod redact;
#[path = "../src/spill.rs"]
mod spill;
use spill::{Spill, Stream};

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
    kept_path(&mut spill)
}

fn kept_path(spill: &mut Spill) -> Result<PathBuf, String> {
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
    // Two outputs of one length on one name: the planted one stays, the new one gets its own.
    fs::remove_file(dir.join(&name))?;
    fs::write(dir.join(&name), "OUTPUT")?;
    let second = kept(&dir, b"output")?;
    assert_eq!(fs::read_to_string(dir.join(&name))?, "OUTPUT");
    assert_eq!(fs::read(&second)?, b"output");
    fs::write(dir.join(&name), "output")?;
    assert_eq!(
        kept(&dir, b"output")?,
        dir.join(&name),
        "equal bytes reuse the name"
    );
    Ok(())
}

/// #881: a linked spill root is refused, so no spill reaches its target.
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

/// Review of #1147: stdout and stderr share one file, and one redactor joined a stdout line left
/// open with the stderr line that closed, so a key's tail arrived as a fresh line in clear.
#[test]
fn a_key_split_across_stdout_reads_is_redacted_whole() -> Fallible {
    let root = Scratch::new("yi-spill-streams")?;
    let mut spill = Spill::new(Some(&root));
    spill.write_to(
        Stream::Out,
        format!("API_KEY=sk-{}-0123456789", "or-v1").as_bytes(),
    );
    spill.write_to(Stream::Err, b"warning: retrying\n");
    spill.write_to(Stream::Out, b"abcdef0123456789abcdef\n");
    let path = kept_path(&mut spill)?;
    let kept = fs::read_to_string(path)?;
    assert!(!kept.contains("0123456789"), "{kept}");
    assert!(kept.contains("warning: retrying\n"), "{kept}");
    Ok(())
}

/// Review of #1147: source code that names a token, and a grep hit on a key's first line, came
/// back mangled; a PGP key block and a digit-less `.env` passphrase came back in clear.
#[test]
fn the_redactor_leaves_code_alone_and_catches_every_key_block() -> Fallible {
    let fence = |end: &str, kind: &str| format!("-----{end} {kind}-----");
    let code = [
        "let max_tokens = budget.remaining_tokens();".to_owned(),
        "api_key = load_api_key_from_environment()".to_owned(),
        "tokenizer = Tokenizer.from_pretrained(\"bert-base-uncased\")".to_owned(),
        format!(
            "src/a.rs:3:const HDR: &str = \"{}\";",
            fence("BEGIN", "RSA PRIVATE KEY")
        ),
        "src/b.rs:9:fn main() {}".to_owned(),
    ];
    let pgp = [
        fence("BEGIN", "PGP PRIVATE KEY BLOCK"),
        "Version: GnuPG v2".to_owned(),
        String::new(),
        "lQOYBGVdB5wBCADKq0nJvCh1Xo3s9mVb".to_owned(),
        "=x0Ab".to_owned(),
        fence("END", "PGP PRIVATE KEY BLOCK"),
    ];
    let env = "export DB_PASSWORD=correcthorsebatterystaple\ndb_password=batterystaplehorsecorrect";
    let source = format!("{}\n{}\n{env}\n", code.join("\n"), pgp.join("\n"));
    let mut out = Vec::new();
    let mut redactor = redact::Redactor::new().ok_or("no redactor")?;
    redactor.push(source.as_bytes(), &mut out);
    redactor.finish(&mut out);
    let kept = String::from_utf8(out)?;
    let lines: Vec<&str> = kept.lines().collect();
    assert_eq!(
        lines.get(..code.len()),
        Some(&code.iter().map(String::as_str).collect::<Vec<_>>()[..])
    );
    assert!(
        !kept.contains("lQOYBGVd") && !kept.contains("=x0Ab"),
        "{kept}"
    );
    assert_eq!(lines.len(), code.len() + pgp.len() + 2, "{kept}");
    assert!(
        !kept.contains("correcthorse"),
        "an upper-case name needs no digit: {kept}"
    );
    assert!(
        !kept.contains("batterystaplehorse"),
        "nor does a `.env` line: {kept}"
    );
    Ok(())
}

/// Review of #1148: both streams ending mid-line had their open lines joined into one at keep.
#[test]
fn each_streams_open_last_line_stays_its_own_line() -> Fallible {
    let root = Scratch::new("yi-spill-tails")?;
    let mut spill = Spill::new(Some(&root));
    spill.write_to(Stream::Out, b"compiled 3 crates\nout tail");
    spill.write_to(Stream::Err, b"err tail");
    let kept = fs::read_to_string(kept_path(&mut spill)?)?;
    assert_eq!(kept, "compiled 3 crates\nout tail\nerr tail");
    Ok(())
}
