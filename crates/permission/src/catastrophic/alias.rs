use std::fs;
use std::io;
use std::path::PathBuf;

use super::{CatastrophicContext, FileId, SEPARATORS, expand, file_id};

#[cfg(unix)]
pub fn links(meta: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(meta)
}

#[cfg(not(unix))]
pub fn links(_meta: &fs::Metadata) -> u64 {
    1
}

/// Store entries one scan reads before it stops and every hard-linked read asks; the stores on
/// a working Mac held 616 (2026-11-11).
const STORE_SCAN_CAP: usize = 10_000;

/// A store file's first name found, its link count, and how many of its names the stores hold.
struct StoreFile {
    path: PathBuf,
    links: u64,
    names: u64,
}

type StoreFiles = std::collections::HashMap<FileId, StoreFile>;

/// Why no hard link to a store file can be ruled out: the scan stopped short of the stores.
enum Unscanned {
    Cap,
    Unreadable(PathBuf, io::ErrorKind),
}

impl std::fmt::Display for Unscanned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cap => write!(
                f,
                "the credential stores hold more than {STORE_SCAN_CAP} entries (STORE_SCAN_CAP), so a hard link to one of their files cannot be ruled out"
            ),
            Self::Unreadable(path, kind) => write!(
                f,
                "{} could not be read ({kind}), so a hard link to a credential store file cannot be ruled out",
                path.display()
            ),
        }
    }
}

/// Invariant: a link inside a store is not followed, as no path rule follows it; a store that is
/// itself a link is, as the sandbox binds its resolved path.
fn scan_stores(stores: &[PathBuf]) -> Result<StoreFiles, Unscanned> {
    let mut files = StoreFiles::new();
    let mut note = |path: PathBuf, meta: &fs::Metadata| {
        if let Some(id) = file_id(meta) {
            let links = links(meta);
            let file = files.entry(id).or_insert(StoreFile {
                path,
                links,
                names: 0,
            });
            file.names = file.names.saturating_add(1);
        }
    };
    let (mut pending, mut walked, mut entries) =
        (Vec::new(), std::collections::HashSet::new(), 0_usize);
    for store in stores {
        match fs::metadata(store) {
            Ok(meta) if meta.is_dir() => pending.push(store.clone()),
            Ok(meta) if meta.is_file() => note(store.clone(), &meta),
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(Unscanned::Unreadable(store.clone(), error.kind()));
            }
            _ => {}
        }
    }
    while let Some(dir) = pending.pop() {
        let unreadable = |error: io::Error| Unscanned::Unreadable(dir.clone(), error.kind());
        let id = file_id(&fs::metadata(&dir).map_err(unreadable)?);
        if !id.is_none_or(|id| walked.insert(id)) {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            entries = entries.saturating_add(1);
            if entries > STORE_SCAN_CAP {
                return Err(Unscanned::Cap);
            }
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) if meta.is_file() => note(entry.path(), &meta),
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(Unscanned::Unreadable(entry.path(), error.kind()));
                }
                _ => {}
            }
        }
    }
    Ok(files)
}

/// Invariant: a hard link names a store file past every path rule, the sandbox's too, which
/// judges the name opened; the stores are scanned at most once per check, and only on demand.
#[derive(Default)]
pub(super) struct Aliases(std::sync::OnceLock<Result<StoreFiles, Unscanned>>);

impl Aliases {
    fn scan(&self, stores: &[PathBuf]) -> &Result<StoreFiles, Unscanned> {
        self.0.get_or_init(|| scan_stores(stores))
    }

    /// Why `meta`, a file with more than one name, may be a store file under another name.
    pub(super) fn names(&self, stores: &[PathBuf], meta: Option<&fs::Metadata>) -> Option<String> {
        let meta = meta.filter(|meta| meta.is_file() && links(meta) > 1)?;
        match self.scan(stores) {
            Ok(files) => (file_id(meta).and_then(|id| files.get(&id)))
                .map(|file| format!("another name for {}", file.path.display())),
            Err(why) => Some(format!("a hard-linked file, and {why}")),
        }
    }

    /// A store file with a name outside every store, which a glob or recursive search can reach.
    fn outside(&self, stores: &[PathBuf]) -> Option<String> {
        match self.scan(stores) {
            Ok(files) => (files.values())
                .filter(|file| file.links > file.names)
                .map(|file| &file.path)
                .min()
                .map(|path| format!("{} has a hard link outside the stores, which a glob or a recursive search can reach", path.display())),
            Err(why) => Some(why.to_string()),
        }
    }
}

const SEARCHERS: [&str; 3] = ["rg", "ag", "ack"];

/// A program that reads a whole tree: rg, ag and ack always, grep with a recursive flag.
fn searches_recursively(command: &str) -> bool {
    let words: Vec<&str> = (command.split(|c: char| c.is_whitespace() || SEPARATORS.contains(&c)))
        .map(|word| word.trim_matches(['"', '\'']))
        .collect();
    let named = |names: &[&str]| {
        (words.iter()).any(|word| names.contains(&word.rsplit('/').next().unwrap_or(word)))
    };
    let recursive = |word: &&str| {
        word.ends_with("recursive")
            || word.ends_with("recurse")
            || (word.strip_prefix('-'))
                .is_some_and(|flags| !flags.starts_with('-') && flags.contains(['r', 'R']))
    };
    named(&SEARCHERS) || (named(&["grep", "egrep", "fgrep"]) && words.iter().any(recursive))
}

fn shell_glob(word: &str) -> bool {
    word.contains(['*', '?'])
        || (word.contains('[') && word.contains(']'))
        || (word.contains('{') && word.contains('}'))
        || ["@(", "+(", "!("].iter().any(|form| word.contains(form))
}

/// A path argument that is a store file by another name, or a glob or recursive search while
/// any store file has a name outside the stores; the target tree is never walked.
pub(super) fn reads_alias(
    command: &str,
    context: &CatastrophicContext,
    stores: &[PathBuf],
) -> Option<String> {
    let aliases = Aliases::default();
    let words: Vec<String> = (command.split_whitespace().skip(1))
        .map(|word| word.replace(['\'', '"', '\\'], ""))
        .collect();
    if (searches_recursively(command) || words.iter().any(|word| shell_glob(word)))
        && let Some(why) = aliases.outside(stores)
    {
        return Some(why);
    }
    (words.iter().filter(|word| !word.starts_with('-'))).find_map(|word| {
        let path = expand(word, context);
        let why = aliases.names(stores, fs::metadata(&path).ok().as_ref())?;
        Some(format!("{} is {why}", path.display()))
    })
}
