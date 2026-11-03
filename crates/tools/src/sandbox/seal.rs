//! Landlock only grants, so Linux cannot deny a path inside a writable root. What the host's next
//! git runs or reads is saved when a profile is built, compared before that git, and put back.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const ROOTS_MAX: usize = 8_192;
const ROOT_FILES_MAX: usize = 1_024;
const ROOT_BYTES_MAX: u64 = 16 << 20;
const DEPTH_MAX: usize = 8;

/// A sealed git file changed, or could not be judged: the host's git does not run this time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SealBroken(String);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Saved {
    Absent,
    File {
        bytes: Vec<u8>,
        mode: u32,
    },
    Link(PathBuf),
    Dir {
        mode: u32,
        entries: BTreeMap<OsString, Saved>,
    },
    /// Past a limit, so nothing can be compared and the host's git refuses.
    TooLarge,
    /// A directory of admin dirs too large to save: a name not listed is one a process made.
    Names(BTreeSet<OsString>),
}

#[derive(Debug)]
struct Root {
    /// A planted link at a git dir would send the restore elsewhere.
    parent_real: Option<PathBuf>,
    saved: Saved,
}

#[derive(Default)]
struct Budget {
    files: usize,
    bytes: u64,
}

fn roots() -> &'static Mutex<BTreeMap<PathBuf, Root>> {
    static ROOTS: OnceLock<Mutex<BTreeMap<PathBuf, Root>>> = OnceLock::new();
    ROOTS.get_or_init(Mutex::default)
}

fn save(path: &Path, budget: &mut Budget, depth: usize) -> Saved {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Saved::Absent;
    };
    let mode = meta.permissions().mode();
    if meta.file_type().is_symlink() {
        return std::fs::read_link(path).map_or(Saved::TooLarge, Saved::Link);
    }
    budget.files = budget.files.saturating_add(1);
    budget.bytes = budget.bytes.saturating_add(meta.len());
    if budget.files > ROOT_FILES_MAX || budget.bytes > ROOT_BYTES_MAX || depth > DEPTH_MAX {
        return Saved::TooLarge;
    }
    if meta.is_file() {
        return std::fs::read(path).map_or(Saved::TooLarge, |bytes| Saved::File { bytes, mode });
    }
    if !meta.is_dir() {
        return Saved::TooLarge;
    }
    let mut entries = BTreeMap::new();
    let Ok(read) = std::fs::read_dir(path) else {
        return Saved::TooLarge;
    };
    for entry in read.flatten() {
        let saved = save(&entry.path(), budget, depth.saturating_add(1));
        if saved == Saved::TooLarge {
            return Saved::TooLarge;
        }
        entries.insert(entry.file_name(), saved);
    }
    Saved::Dir { mode, entries }
}

fn names(dir: &Path) -> BTreeSet<OsString> {
    let read = std::fs::read_dir(dir).into_iter().flatten().flatten();
    read.map(|entry| entry.file_name()).collect()
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => Ok(()),
    }
}

fn put_back(path: &Path, saved: &Saved) -> std::io::Result<()> {
    if let Saved::Names(listed) = saved {
        for name in names(path).difference(listed) {
            remove(&path.join(name))?;
        }
        return Ok(());
    }
    remove(path)?;
    match saved {
        Saved::File { bytes, mode } => {
            std::fs::write(path, bytes)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))
        }
        Saved::Link(target) => std::os::unix::fs::symlink(target, path),
        Saved::Dir { mode, entries } => {
            std::fs::create_dir(path)?;
            for (name, entry) in entries {
                put_back(&path.join(name), entry)?;
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))
        }
        Saved::Absent | Saved::TooLarge | Saved::Names(_) => Ok(()),
    }
}

fn current(path: &Path, like: &Saved) -> Saved {
    match like {
        Saved::Names(listed) => {
            let found = names(path);
            match found.is_subset(listed) {
                true => Saved::Names(listed.clone()),
                false => Saved::Names(found),
            }
        }
        _ => save(path, &mut Budget::default(), 0),
    }
}

fn root_of(path: &Path, saved: Saved) -> Root {
    let parent_real = path.parent().and_then(|up| up.canonicalize().ok());
    Root { parent_real, saved }
}

/// Files and links saved whole; `names` directories by their listing. A path already sealed keeps
/// what it holds: only a host writer moves it, by [`reseal`].
pub(super) fn seal(files: &[PathBuf], listed: &[PathBuf]) {
    let Ok(mut held) = roots().lock() else {
        return;
    };
    let fresh = |held: &BTreeMap<PathBuf, Root>, path: &PathBuf| !held.contains_key(path);
    for path in files {
        if fresh(&held, path) {
            let saved = save(path, &mut Budget::default(), 0);
            held.insert(path.clone(), root_of(path, saved));
        }
    }
    for path in listed {
        if fresh(&held, path) {
            held.insert(path.clone(), root_of(path, Saved::Names(names(path))));
        }
    }
}

/// Every sealed root, since a host git in one tree reads a config another tree's process could write.
/// Invariant: a root whose directory is gone belongs to a retired tree and leaves the seal.
pub fn verify() -> Result<(), SealBroken> {
    let mut held =
        (roots().lock()).map_err(|_| SealBroken("the seal's lock is poisoned".to_owned()))?;
    if held.len() > ROOTS_MAX {
        return Err(SealBroken(format!(
            "more git files than the seal watches (seal.roots {ROOTS_MAX}); restart Yi to clear retired trees"
        )));
    }
    held.retain(|path, _| path.parent().is_some_and(Path::exists));
    let mut changed = Vec::new();
    for (path, root) in held.iter() {
        if root.saved == Saved::TooLarge {
            return Err(SealBroken(format!(
                "{} is past what the seal saves (seal.root_files {ROOT_FILES_MAX}, seal.root_bytes {ROOT_BYTES_MAX}), so no host git runs here",
                path.display()
            )));
        }
        if current(path, &root.saved) == root.saved {
            continue;
        }
        let parent_now = path.parent().and_then(|up| up.canonicalize().ok());
        if parent_now != root.parent_real {
            return Err(SealBroken(format!(
                "{} moved since this session sealed it, so no host git runs here",
                path.display()
            )));
        }
        put_back(path, &root.saved).map_err(|error| {
            SealBroken(format!(
                "{} changed and could not be restored: {error}",
                path.display()
            ))
        })?;
        changed.push(path.display().to_string());
    }
    match changed.is_empty() {
        true => Ok(()),
        false => Err(SealBroken(format!(
            "a process changed what the host's git reads: {}; restored, and this git did not run",
            changed.join(", ")
        ))),
    }
}

/// After a host git that writes these itself. Invariant: what a contained process wrote in the moment
/// since [`verify`] is blessed with it, a window one command long.
pub fn reseal() {
    let Ok(mut held) = roots().lock() else {
        return;
    };
    for (path, root) in held.iter_mut() {
        root.saved = match root.saved {
            Saved::Names(_) => Saved::Names(names(path)),
            _ => save(path, &mut Budget::default(), 0),
        };
    }
}

/// `git <verb>` that writes a sealed file when the host itself runs it.
#[must_use]
pub fn writes_sealed(args: &[&str]) -> bool {
    const WRITERS: [&str; 10] = [
        "config",
        "branch",
        "worktree",
        "submodule",
        "remote",
        "init",
        "clone",
        "push",
        "checkout",
        "switch",
    ];
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match *word {
            "-c" | "-C" => drop(words.next()),
            flag if flag.starts_with('-') => {}
            verb => return WRITERS.contains(&verb),
        }
    }
    false
}
