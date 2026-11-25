//! The working folder on disk: its tree, ignore rules kept, and the text of files asked for.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use yd_core::feed::{FILES, FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::{FeedId, RowId};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::{Row, Value};

use crate::git::{GitError, git};

pub const FEED: FeedId = FILES;

/// The most of one file's text a row carries; past it the row says `truncated`.
pub const MAX_TEXT: usize = 2 << 20;
/// Entries one folder lists; a folder past it lists its first ones by name.
pub const MAX_CHILDREN: usize = 10_000;
/// Entries a walk of a folder that is not a repository lists in all, and how deep it goes.
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_DEPTH: usize = 16;
/// Files kept re-read on each poll; asking for one more drops the oldest.
pub const KEPT: usize = 8;
/// How often the disk is read again: nothing is watched.
pub const POLL: Duration = Duration::from_secs(2);

/// The rows a source last sent, so a fresh reading goes out as only what changed.
#[derive(Debug, Default)]
pub struct Mirror(BTreeMap<RowId, Row>);

impl Mirror {
    /// Sends what differs between the rows sent last and `rows`.
    pub fn publish(&mut self, out: &FeedTx, rows: Vec<Row>) -> Result<(), Closed> {
        let fresh: BTreeMap<RowId, Row> = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        let first = self.0.is_empty();
        for gone in self.0.keys().filter(|id| !fresh.contains_key(*id)) {
            out.send(FeedEvent::Remove(gone.clone()))?;
        }
        for (id, row) in &fresh {
            if self.0.get(id) != Some(row) {
                out.send(FeedEvent::Upsert(row.clone()))?;
            }
        }
        if first && fresh.is_empty() {
            out.send(FeedEvent::Status(FeedStatus::Empty))?;
        }
        self.0 = fresh;
        Ok(())
    }

    /// Forgets every row, removing them from the table: the folder changed.
    pub fn clear(&mut self, out: &FeedTx) -> Result<(), Closed> {
        out.send(FeedEvent::Reset(Vec::new()))?;
        self.0.clear();
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&Row> {
        self.0.get(&RowId::new(id))
    }
}

/// `rel` under `root`, or none when it names a place outside it.
pub fn inside(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let (root, at) = (
        root.canonicalize().ok()?,
        root.join(rel).canonicalize().ok()?,
    );
    at.starts_with(&root).then_some(at)
}

/// At most `max` bytes of `bytes`, ending on a character boundary when the text is UTF-8.
pub fn prefix(bytes: &[u8], max: usize) -> &[u8] {
    let head = bytes.get(..max).unwrap_or(bytes);
    match std::str::from_utf8(head) {
        Err(cut) if cut.error_len().is_none() => head.get(..cut.valid_up_to()).unwrap_or(head),
        _ => head,
    }
}

/// Invariant: nothing is read until `SourceCmd::Root` names a folder, since a Finder launch
/// starts in `/`; then the tree and the files asked for are read again on every poll.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let mut root: Option<PathBuf> = None;
    let mut also: Option<PathBuf> = None;
    let mut reads = VecDeque::new();
    let mut mirror = Mirror::default();
    loop {
        if let Some(root) = &root {
            mirror.publish(&out, rows(root, also.as_deref(), &reads))?;
        }
        match cmds.recv_timeout(POLL) {
            Ok(SourceCmd::Root(folder)) => {
                if moved(&mut root, folder) {
                    reads.clear();
                    mirror.clear(&out)?;
                }
            }
            Ok(SourceCmd::Read(path)) => {
                if let Some(path) = path.to_str() {
                    keep(&mut reads, path.to_owned());
                }
            }
            Ok(SourceCmd::Readable(folder)) => also = folder.is_absolute().then_some(folder),
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Puts `path` last among the files re-read, dropping the oldest past [`KEPT`].
pub fn keep(reads: &mut VecDeque<String>, path: String) {
    reads.retain(|p| *p != path);
    reads.push_back(path);
    while reads.len() > KEPT {
        reads.pop_front();
    }
}

/// Points a source at `folder`; true when that changed where it reads. Invariant: a folder
/// that is not absolute is refused, since it would be read from wherever the app started.
pub fn moved(root: &mut Option<PathBuf>, folder: PathBuf) -> bool {
    if !folder.is_absolute() || root.as_ref() == Some(&folder) {
        return false;
    }
    *root = Some(folder);
    true
}

/// Every folder and file under `root` and a row for each file asked for; `also` is the one
/// other folder a file may be read from.
pub fn rows(root: &Path, also: Option<&Path>, reads: &VecDeque<String>) -> Vec<Row> {
    let mut rows = tree(root);
    rows.extend(reads.iter().map(|path| read(root, also, path)));
    rows
}

/// An absolute `path` when it is a file under `also`, links followed: the one way out of the
/// working folder, and only to read.
fn beside(also: Option<&Path>, path: &str) -> Option<PathBuf> {
    let (also, at) = (also?.canonicalize().ok()?, Path::new(path));
    let at = at.is_absolute().then(|| at.canonicalize().ok())??;
    at.starts_with(&also).then_some(at)
}

/// The tree as rows: id the path, parent the folder, cells `name` and `kind`; a row `tree`
/// with `truncated` and `cut` when the walk stopped short.
pub fn tree(root: &Path) -> Vec<Row> {
    let (files, cut) = listed(root).map_or_else(|_| walk(root), |files| (files, false));
    entries(&files, cut)
}

fn entries(files: &[String], cut: bool) -> Vec<Row> {
    let mut folders = BTreeSet::new();
    let mut rows = Vec::new();
    for file in files {
        let mut at = file.as_str();
        while let Some((folder, _)) = at.rsplit_once('/') {
            if !folders.insert(folder.to_owned()) {
                break;
            }
            at = folder;
        }
        rows.push(entry(file, "file"));
    }
    rows.extend(folders.iter().map(|folder| entry(folder, "folder")));
    if cut {
        let why = format!(
            "{} files shown: the folder is not a repository, and its walk stops at {MAX_ENTRIES} \
             entries, {MAX_DEPTH} folders deep, {MAX_CHILDREN} per folder",
            files.len()
        );
        rows.push(
            Row::new("tree")
                .cell("truncated", Value::Flag(true))
                .text("cut", why),
        );
    }
    rows
}

fn entry(path: &str, kind: &str) -> Row {
    let (parent, name) = path
        .rsplit_once('/')
        .map_or((None, path), |(p, n)| (Some(p), n));
    let mut row = Row::new(path).text("name", name).text("kind", kind);
    row.parent = parent.map(RowId::new);
    row
}

/// The files git sees under `root`, ignore rules applied: tracked and untracked.
fn listed(root: &Path) -> Result<Vec<String>, GitError> {
    let args = [
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "--deduplicate",
        "-z",
    ];
    let out = git(root, &args, b"")?;
    let text = String::from_utf8_lossy(&out);
    let mut files: Vec<String> = text
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    files.retain(|path| root.join(path).exists());
    Ok(files)
}

/// The files under a folder that is not a repository, `.git` folders left out, and whether
/// [`MAX_ENTRIES`], [`MAX_DEPTH`] or [`MAX_CHILDREN`] cut the walk.
fn walk(root: &Path) -> (Vec<String>, bool) {
    let (mut files, mut seen, mut cut) = (Vec::new(), 0usize, false);
    let mut folders = vec![(PathBuf::new(), 0usize)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&folder)) else {
            continue;
        };
        let mut names: Vec<_> = entries.filter_map(Result::ok).take(MAX_ENTRIES).collect();
        names.sort_by_key(std::fs::DirEntry::file_name);
        cut |= names.len() > MAX_CHILDREN;
        for item in names.into_iter().take(MAX_CHILDREN) {
            if seen >= MAX_ENTRIES {
                return (files, true);
            }
            seen = seen.saturating_add(1);
            let (name, path) = (item.file_name(), folder.join(item.file_name()));
            match item.file_type() {
                Ok(kind) if kind.is_dir() && name != ".git" && depth < MAX_DEPTH => {
                    folders.push((path, depth.saturating_add(1)));
                }
                Ok(kind) if kind.is_dir() && name != ".git" => cut = true,
                Ok(kind) if kind.is_file() => files.push(path.to_string_lossy().into_owned()),
                _ => {}
            }
        }
    }
    (files, cut)
}

/// The row `file:<path>`: the text, cut at [`MAX_TEXT`], and its line count.
pub fn read(root: &Path, also: Option<&Path>, path: &str) -> Row {
    let row = Row::new(["file:", path].concat()).text("path", path);
    let Some(at) = inside(root, path).or_else(|| beside(also, path)) else {
        return row.text("error", "the path is outside the working folder");
    };
    let mut bytes = Vec::new();
    let opened =
        std::fs::File::open(at).and_then(|f| f.take(MAX_TEXT as u64 + 1).read_to_end(&mut bytes));
    if let Err(why) = opened {
        return row.text("error", why.to_string());
    }
    let cut = bytes.len() > MAX_TEXT;
    let text = String::from_utf8_lossy(prefix(&bytes, MAX_TEXT)).into_owned();
    let lines = text.lines().count();
    let row = row.text("text", text).text("lines", lines.to_string());
    if cut {
        row.cell("truncated", Value::Flag(true))
    } else {
        row
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::error::Error;
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    /// A fresh scratch folder under the workspace's `target/`.
    pub(crate) fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let at = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/fc-scratch")
            .join(name);
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at)?;
        Ok(at)
    }

    /// Whether this git cannot list worktrees with `-z` (new in 2.36), which it says: the test has
    /// nothing to run. Incident: the forge's git is 2.34. Any other failure is the test's own.
    pub(crate) fn old_git() -> Result<bool, Box<dyn Error>> {
        static OLD: std::sync::OnceLock<Result<bool, String>> = std::sync::OnceLock::new();
        let probe = || -> Result<bool, Box<dyn Error>> {
            let at = scratch("git-z")?;
            sh(&at, &["init", "-q"])?;
            #[allow(clippy::disallowed_methods, reason = "a test asks git what it can do")]
            let out = Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&at)
                .args(["worktree", "list", "--porcelain", "-z"])
                .output()?;
            let said = String::from_utf8_lossy(&out.stderr);
            match (out.status.success(), said.contains("unknown switch")) {
                (true, _) => Ok(false),
                (false, true) => Ok(true),
                (false, false) => Err(format!("git worktree list -z failed: {said}").into()),
            }
        };
        let old = OLD
            .get_or_init(|| probe().map_err(|e| e.to_string()))
            .clone()?;
        if old {
            eprintln!("skipped: this git has no `worktree list -z`, new in 2.36");
        }
        Ok(old)
    }

    /// `git` in `at`, for setting a scratch repository up.
    pub(crate) fn sh(at: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
        #[allow(
            clippy::disallowed_methods,
            reason = "a test makes its scratch repository"
        )]
        let ok = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(at)
            .args(args)
            .output()?
            .status
            .success();
        if ok {
            Ok(())
        } else {
            Err(format!("git {args:?} failed").into())
        }
    }

    #[test]
    fn a_source_moves_only_to_a_new_absolute_folder() {
        let mut root = None;
        assert!(!moved(&mut root, PathBuf::from("atlas-api")), "relative");
        assert_eq!(root, None);
        assert!(moved(&mut root, PathBuf::from("/work/atlas-api")));
        assert!(!moved(&mut root, PathBuf::from("/work/atlas-api")), "same");
        assert!(moved(&mut root, PathBuf::from("/work/storefront")));
        assert_eq!(root, Some(PathBuf::from("/work/storefront")));
    }

    #[test]
    fn the_tree_keeps_ignore_rules_and_names_each_parent() -> Result<(), Box<dyn Error>> {
        let at = scratch("tree")?;
        sh(&at, &["init", "-q"])?;
        std::fs::create_dir_all(at.join("src/middleware"))?;
        std::fs::create_dir_all(at.join("target"))?;
        std::fs::write(at.join(".gitignore"), "target/\n")?;
        std::fs::write(at.join("src/middleware/rate_limit.rs"), "fn a() {}\n")?;
        std::fs::write(at.join("target/junk.o"), "x")?;
        let rows = tree(&at);
        let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains("src/middleware/rate_limit.rs"), "{ids:?}");
        assert!(
            ids.contains("src") && ids.contains("src/middleware"),
            "{ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.starts_with("target")),
            "ignored files listed: {ids:?}"
        );
        let file = rows
            .iter()
            .find(|r| r.id.as_str() == "src/middleware/rate_limit.rs");
        assert_eq!(
            file.and_then(|r| r.parent.clone()),
            Some(RowId::new("src/middleware"))
        );
        Ok(())
    }

    #[test]
    fn a_read_is_cut_at_the_cap_and_says_so() -> Result<(), Box<dyn Error>> {
        let at = scratch("read")?;
        std::fs::write(at.join("small.rs"), "one\ntwo\n")?;
        std::fs::write(at.join("big.txt"), ["a", &"é".repeat(MAX_TEXT)].concat())?;
        let small = read(&at, None, "small.rs");
        assert_eq!(
            (small.words("text"), small.words("lines")),
            (Some("one\ntwo\n"), Some("2"))
        );
        assert_eq!(small.get("truncated"), None);
        let big = read(&at, None, "big.txt");
        assert_eq!(big.get("truncated"), Some(&Value::Flag(true)));
        let text = big.words("text").ok_or("no text")?;
        assert!(!text.contains('\u{FFFD}'), "the cut split a character");
        assert_eq!(
            text.len(),
            MAX_TEXT - 1,
            "cut on the boundary before the cap"
        );
        Ok(())
    }

    #[test]
    fn a_walk_past_its_caps_stops_and_says_so() -> Result<(), Box<dyn Error>> {
        let at = scratch("walk")?;
        let deep = (0..=MAX_DEPTH).fold(at.clone(), |p, i| p.join(format!("d{i}")));
        std::fs::create_dir_all(&deep)?;
        std::fs::write(deep.join("below.txt"), "x")?;
        std::fs::write(at.join("top.txt"), "x")?;
        let (files, cut) = walk(&at);
        let rows = entries(&files, cut);
        assert!(rows.iter().any(|r| r.id.as_str() == "top.txt"));
        assert!(!rows.iter().any(|r| r.id.as_str().ends_with("below.txt")));
        let cut = rows
            .iter()
            .find(|r| r.id.as_str() == "tree")
            .ok_or("no cut row")?;
        assert_eq!(cut.get("truncated"), Some(&Value::Flag(true)));
        assert!(
            cut.words("cut")
                .is_some_and(|w| w.contains("1 files shown"))
        );
        Ok(())
    }

    #[test]
    fn the_files_re_read_are_the_last_few_asked_for() {
        let mut reads = VecDeque::new();
        for n in 0..=KEPT {
            keep(&mut reads, n.to_string());
        }
        keep(&mut reads, "3".to_owned());
        assert_eq!(reads.len(), KEPT);
        assert_eq!(reads.front().map(String::as_str), Some("1"));
        assert_eq!(reads.back().map(String::as_str), Some("3"));
    }

    #[test]
    fn a_read_outside_the_folder_is_refused() -> Result<(), Box<dyn Error>> {
        let at = scratch("escape")?;
        std::fs::write(at.join("../secret.txt"), "no")?;
        for path in ["../secret.txt", "/etc/hosts"] {
            let row = read(&at, None, path);
            assert_eq!(row.get("text"), None, "{path} was read");
            assert!(row.words("error").is_some());
        }
        Ok(())
    }
    /// The app's panes folder is the one other place a file is read from: a plugin's source
    /// opens, while its neighbour outside the folder and a link out of it do not.
    #[test]
    fn a_file_under_the_second_root_is_read_and_nothing_else_outside_is()
    -> Result<(), Box<dyn Error>> {
        let (at, base) = (scratch("second-root")?, scratch("second-base")?);
        let panes = base.join("panes");
        std::fs::create_dir_all(panes.join("queue"))?;
        std::fs::write(panes.join("queue/pane.luau"), "return {}\n")?;
        std::fs::write(base.join("settings.json"), "{}")?;
        std::os::unix::fs::symlink(base.join("settings.json"), panes.join("link.json"))?;
        let also = Some(panes.as_path());
        let path = |rest: &str| base.join(rest).display().to_string();
        let source = read(&at, also, &path("panes/queue/pane.luau"));
        assert_eq!(source.words("text"), Some("return {}\n"));
        for outside in ["settings.json", "panes/link.json", "panes/../settings.json"] {
            let row = read(&at, also, &path(outside));
            assert_eq!(row.get("text"), None, "{outside} was read");
        }
        let unnamed = read(&at, None, &path("panes/queue/pane.luau"));
        assert_eq!(unnamed.get("text"), None, "read with no second root named");
        Ok(())
    }
}
