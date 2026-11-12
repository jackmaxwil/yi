//! `ReadGate` judges the file it opened (#890): `d` turns into a link to the key store at the
//! one moment a racing process could use, between the gate's judgement and its act, every run.
#![cfg(unix)]

use std::error::Error;
use std::io;
use std::path::PathBuf;

use yi_permission::{CatastrophicContext, ReadGate, command_reads_alias};

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
        let (scratch, context) = keyed(name)?;
        let (store, d) = (scratch.join("home/.ssh"), scratch.join("workspace/d"));
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join("id_rsa"), "ordinary\n")?;
        let gate = ReadGate::new(&context);
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

/// `real/proj/.git` is the repo and `a/link` points at `real/inner`, so `a/link/../proj` is
/// `real/proj` to the kernel and a missing `a/proj` to a lexical pass.
fn linked_repo(scratch: &Scratch) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let git = scratch.join("real/proj/.git");
    std::fs::create_dir_all(&git)?;
    std::fs::create_dir_all(scratch.join("real/inner"))?;
    std::fs::create_dir_all(scratch.join("a"))?;
    std::fs::write(git.join("config"), "[core]\n")?;
    std::os::unix::fs::symlink(scratch.join("real/inner"), scratch.join("a/link"))?;
    Ok((
        scratch.join("a/link/../proj"),
        git.join("config").canonicalize()?,
    ))
}

fn opens(cwd: &std::path::Path, config: &std::path::Path) -> bool {
    let gate = ReadGate::new(&CatastrophicContext::detect(cwd));
    gate.open(config, &[]).is_ok()
}

#[test]
fn a_cwd_spelled_through_a_link_and_dotdot_keeps_its_git_dir_walled() -> TestResult {
    let scratch = Scratch::new("yi-open-dotdot")?;
    let (cwd, config) = linked_repo(&scratch)?;
    assert_eq!(cwd.join(".git/config").canonicalize()?, config);
    assert!(
        !opens(&cwd, &config),
        "read {} through the wall",
        config.display()
    );
    Ok(())
}

/// `home/.ssh/id_rsa` holds the key, beside an empty `workspace`.
pub(crate) fn keyed(name: &str) -> Result<(Scratch, CatastrophicContext), Box<dyn Error>> {
    let scratch = Scratch::new(name)?;
    let (home, workspace) = (scratch.join("home"), scratch.join("workspace"));
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(home.join(".ssh/id_rsa"), KEY)?;
    let context = CatastrophicContext {
        home_dir: Some(home),
        working_dir: Some(workspace),
        workspace_git: Vec::new(),
        host_owned: Vec::new(),
    };
    Ok((scratch, context))
}

#[test]
fn a_hard_link_to_a_key_file_is_refused() -> TestResult {
    let (scratch, context) = keyed("yi-gate-hardlink")?;
    let (key, alias) = (
        scratch.join("home/.ssh/id_rsa"),
        scratch.join("workspace/key-copy"),
    );
    std::fs::hard_link(&key, &alias)?;
    let (own, twin) = (
        scratch.join("workspace/own"),
        scratch.join("workspace/twin"),
    );
    std::fs::write(&own, "ordinary\n")?;
    std::fs::hard_link(&own, &twin)?;
    let gate = ReadGate::new(&context);

    let why = gate.open(&alias, &[]).err().map(|error| error.to_string());
    let why = why.unwrap_or_default();
    assert!(
        why.contains("another name for"),
        "ReadGate::open read the alias: {why}"
    );
    if let Ok(mut file) = gate.open_write(&alias, &[]) {
        std::io::Write::write_all(&mut file, b"OVERWRITTEN\n")?;
    }
    assert_eq!(
        std::fs::read_to_string(&key)?,
        KEY,
        "the alias changed the key"
    );
    assert_eq!(io::read_to_string(gate.open(&twin, &[])?)?, "ordinary\n");
    Ok(())
}

/// The stores are read whole to the cap and no further, and a cut is named in the refusal.
#[test]
fn a_store_scan_reads_to_its_cap_and_names_it_past_that() -> TestResult {
    let (scratch, context) = keyed("yi-gate-store-cap")?;
    let (ssh, alias) = (
        scratch.join("home/.ssh"),
        scratch.join("workspace/key-copy"),
    );
    std::fs::hard_link(ssh.join("id_rsa"), &alias)?;
    // Ten thousand entries with the link, whose two files a scan that followed it would add.
    std::fs::create_dir_all(scratch.join("elsewhere"))?;
    std::fs::write(scratch.join("elsewhere/a"), "")?;
    std::fs::write(scratch.join("elsewhere/b"), "")?;
    std::os::unix::fs::symlink(scratch.join("elsewhere"), ssh.join("elsewhere"))?;
    for n in 1..9_999 {
        std::fs::write(ssh.join(format!("known_{n}")), "")?;
    }
    let judged = |command: &str| command_reads_alias(command, &context).unwrap_or_default();
    let refusals = || {
        let opened = ReadGate::new(&context).open(&alias, &[]).err();
        let opened = opened.map(|error| error.to_string()).unwrap_or_default();
        [judged("cat key-copy"), opened, judged("rg token")]
    };

    let at_cap = refusals();
    let named = |why: &String| why.contains("id_rsa") && !why.contains("STORE_SCAN_CAP");
    assert!(at_cap.iter().all(named), "{at_cap:?}");
    std::fs::write(ssh.join("known_9999"), "")?;
    let [named_read, opened, search] = refusals();
    let cut = "more than 10000 entries (STORE_SCAN_CAP)";
    assert!(
        named_read.contains(cut) && opened.contains(cut),
        "{named_read} / {opened}"
    );
    // A cut scan names no alias, so only a path with two names asks.
    assert_eq!(search, "", "a cut scan made rg ask");
    Ok(())
}

/// A store that cannot be read is named where a two-name file is refused; it never gates rg.
#[test]
fn an_unreadable_store_is_named_and_asks_only_for_a_two_name_path() -> TestResult {
    let (scratch, context) = keyed("yi-gate-store-loop")?;
    // A link to itself fails with ELOOP for root too, as a chmod fixture would not.
    std::os::unix::fs::symlink(".aws", scratch.join("home/.aws"))?;
    let (own, twin) = (
        scratch.join("workspace/own"),
        scratch.join("workspace/twin"),
    );
    std::fs::write(&own, "ordinary\n")?;
    std::fs::hard_link(&own, &twin)?;
    std::fs::create_dir_all(scratch.join("workspace/nested"))?;
    std::fs::hard_link(&own, scratch.join("workspace/nested/inner"))?;
    std::fs::write(scratch.join("workspace/single"), "ordinary\n")?;
    let judged = |command: &str| command_reads_alias(command, &context).unwrap_or_default();
    let opened = ReadGate::new(&context).open(&twin, &[]).err();
    let opened = opened.map(|error| error.to_string()).unwrap_or_default();

    let unread = format!("{} could not be read", scratch.join("home/.aws").display());
    let spellings = [
        "cat twin",
        "cd nested && cat inner",
        "cat <twin",
        "cat < twin",
        "head -c 1 <twin",
        "cat <>twin",
        "cat twin>/tmp/k",
        "cat x>twin",
        "echo $(cat twin)",
        "echo `cat twin`",
        "{ cat twin; }",
        "cat \"twin\" 2>/dev/null",
    ];
    for (spelled, why) in spellings
        .iter()
        .map(|c| (*c, judged(c)))
        .chain([("open", opened)])
    {
        assert!(why.contains(&unread), "{spelled}: {why}");
    }
    assert_eq!([judged("rg token"), judged("jq '.[]' single")], ["", ""]);
    let single = ReadGate::new(&context).open(&scratch.join("workspace/single"), &[]);
    assert_eq!(io::read_to_string(single?)?, "ordinary\n");
    Ok(())
}

#[test]
fn a_hard_link_swapped_in_after_the_judgement_writes_no_key() -> TestResult {
    let (scratch, context) = keyed("yi-gate-hardlink-swap")?;
    let (key, target) = (
        scratch.join("home/.ssh/id_rsa"),
        scratch.join("workspace/copy"),
    );
    std::fs::write(&target, "ordinary\n")?;
    let mut swapped = None;
    let opened = ReadGate::new(&context).open_write_after(&target, &[], &mut || {
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
fn a_relative_cwd_keeps_its_git_dir_walled() -> TestResult {
    let scratch = Scratch::new("yi-open-relative")?;
    let (_, config) = linked_repo(&scratch)?;
    let up: PathBuf = (std::env::current_dir()?.components().skip(1))
        .map(|_| "..")
        .collect();
    let cwd = up.join(scratch.join("real/proj").strip_prefix("/")?);
    assert!(cwd.is_relative() && cwd.join(".git/config").canonicalize()? == config);
    assert!(
        !opens(&cwd, &config),
        "read {} through the wall",
        config.display()
    );
    Ok(())
}
