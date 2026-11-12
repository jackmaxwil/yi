use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{CatastrophicContext, FileId, cd_target, expand, file_id, stores};

/// How many directory entries name one file.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Links(u64);

impl Links {
    #[must_use]
    pub fn shared(self) -> bool {
        self.0 > 1
    }
}

#[cfg(unix)]
pub fn links(meta: &fs::Metadata) -> Links {
    Links(std::os::unix::fs::MetadataExt::nlink(meta))
}

#[cfg(not(unix))]
pub fn links(_meta: &fs::Metadata) -> Links {
    Links(1)
}

/// Incident: each relative `cd` resolved against every dir so far, doubling them, so a chain of
/// k cds cost 2^k resolves per word; past this a check refuses rather than skip a target.
pub const CD_DIRS_MAX: usize = 64;

/// Store entries one scan reads before it stops and every hard-linked read asks; the stores on
/// a working Mac held 616 (2026-11-11).
const STORE_SCAN_CAP: usize = 10_000;

/// A store file's first name found, its link count, and how many of its names the stores hold.
struct StoreFile {
    path: PathBuf,
    links: Links,
    names: Links,
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
    let (mut files, mut seen) = (StoreFiles::new(), std::collections::HashSet::new());
    // Incident: a store linked to a file in another store counted that one name twice.
    let mut note = |path: PathBuf, real: PathBuf, meta: &fs::Metadata| {
        if let Some(id) = file_id(meta) {
            let links = links(meta);
            let file = files.entry(id).or_insert(StoreFile {
                path,
                links,
                names: Links(0),
            });
            if seen.insert(real) {
                file.names = Links(file.names.0.saturating_add(1));
            }
        }
    };
    let (mut pending, mut walked, mut entries) =
        (Vec::new(), std::collections::HashSet::new(), 0_usize);
    for store in stores {
        match fs::metadata(store) {
            Ok(meta) if meta.is_dir() => pending.push(store.clone()),
            Ok(meta) if meta.is_file() => {
                let unreadable =
                    |error: io::Error| Unscanned::Unreadable(store.clone(), error.kind());
                note(
                    store.clone(),
                    fs::canonicalize(store).map_err(unreadable)?,
                    &meta,
                );
            }
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
        let real = fs::canonicalize(&dir).map_err(unreadable)?;
        for entry in fs::read_dir(&dir).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            entries = entries.saturating_add(1);
            if entries > STORE_SCAN_CAP {
                return Err(Unscanned::Cap);
            }
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) if meta.is_file() => {
                    note(entry.path(), real.join(entry.file_name()), &meta)
                }
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
/// judges the name opened; scanned once per check or gate, never kept, so a new link is seen.
#[derive(Default)]
pub(super) struct Aliases(std::sync::OnceLock<Result<StoreFiles, Unscanned>>);

impl Aliases {
    fn scan(&self, stores: &[PathBuf]) -> &Result<StoreFiles, Unscanned> {
        self.0.get_or_init(|| scan_stores(stores))
    }

    /// Why `meta`, a file with more than one name, may be a store file under another name.
    pub(super) fn names(&self, stores: &[PathBuf], meta: Option<&fs::Metadata>) -> Option<String> {
        let meta = meta.filter(|meta| meta.is_file() && links(meta).shared())?;
        match self.scan(stores) {
            Ok(files) => (file_id(meta).and_then(|id| files.get(&id)))
                .map(|file| format!("another name for {}", file.path.display())),
            Err(why) => Some(format!("a hard-linked file, and {why}")),
        }
    }

    /// A store file with a name outside every store; a cut scan claims none, since every
    /// path that has two names still asks through [`Aliases::names`].
    fn outside(&self, stores: &[PathBuf]) -> Option<String> {
        let files = self.scan(stores).as_ref().ok()?;
        (files.values())
            .filter(|file| file.links > file.names)
            .map(|file| &file.path)
            .min()
            .map(|path| format!("{} has a hard link outside the stores, which the sandbox cannot see, so every command may read it", path.display()))
    }

    pub(super) fn cut(&self) -> Option<String> {
        self.0.get()?.as_ref().err().map(Unscanned::to_string)
    }
}

/// Invariant: every command asks while a store file has a name outside the stores, as the
/// sandbox matches the name opened; a cut scan leaves only a named two-name path asking.
pub fn command_reads_alias(command: &str, context: &CatastrophicContext) -> Option<String> {
    let (stores, aliases) = (stores(context), Aliases::default());
    // Every `cd` target joins the dirs a later word resolves from, as the wall's check does.
    let (mut dirs, mut after_cd) = (Vec::<PathBuf>::new(), false);
    for word in crate::safety::command_segments(command)
        .into_iter()
        .flatten()
    {
        let at = expand(&word, context);
        if cd_target(&word, &mut after_cd) && !dirs.contains(&at) {
            dirs.push(at.clone());
            if dirs.len() > CD_DIRS_MAX {
                return Some(format!(
                    "the command has more than {CD_DIRS_MAX} cd targets (CD_DIRS_MAX), past which no path in it is judged; run the cds as separate calls"
                ));
            }
        }
        let relative = Path::new(&word).is_relative() && !word.starts_with('~');
        let elsewhere = (dirs.iter().filter(|_| relative)).map(|dir| dir.join(&word));
        for path in std::iter::once(at)
            .chain(elsewhere)
            .filter(|_| !word.starts_with('-'))
        {
            if let Some(why) = aliases.names(&stores, fs::metadata(&path).ok().as_ref()) {
                return Some(format!("{} is {why}", path.display()));
            }
        }
    }
    aliases.outside(&stores)
}
