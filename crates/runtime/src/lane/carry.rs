//! What a fresh tree borrows from its parent at first use: cargo units that name nothing in the
//! tree, and the ignored files `.worktreeinclude` lists. Its one rule: a doubt means a rebuild.

use std::collections::HashSet;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const DEP_INFO_MAX_BYTES: u64 = 4 << 20;
const DEP_INFO_FILES_MAX: usize = 65_536;
const INCLUDE_LIST_MAX_BYTES: usize = 64 << 10;
const INCLUDE_PATTERNS_MAX: usize = 256;
const INCLUDE_FILES_MAX: usize = 1_024;
const INCLUDE_BYTES_MAX: u64 = 1 << 30;

/// The paths a dep-info file's rules name, unescaped, as raw bytes: a path need not be UTF-8.
#[must_use]
pub fn dep_info(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut paths = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        // `target: dep dep`, and `# env-dep` rows and blank lines name no path.
        if line.is_empty() || line.first() == Some(&b'#') {
            continue;
        }
        let Some(colon) = find_rule_colon(line) else {
            continue;
        };
        let (mut word, mut escaped) = (Vec::new(), false);
        for byte in line.iter().skip(colon.saturating_add(1)) {
            match (byte, escaped) {
                (b'\\', false) => escaped = true,
                (b' ', false) => {
                    if !word.is_empty() {
                        paths.push(std::mem::take(&mut word));
                    }
                }
                (byte, _) => {
                    word.push(*byte);
                    escaped = false;
                }
            }
        }
        if !word.is_empty() {
            paths.push(word);
        }
    }
    paths
}

/// The first `:` followed by a space or the end: a Windows drive or an escaped colon is neither.
fn find_rule_colon(line: &[u8]) -> Option<usize> {
    (line.iter().enumerate())
        .find(|(at, byte)| {
            **byte == b':' && matches!(line.get(at.saturating_add(1)), None | Some(b' '))
        })
        .map(|(at, _)| at)
}

/// A rule path relative to the build, or absolute under the source root and outside `target/`,
/// bakes this tree into the unit. Incident: a clone at a new path silently tests its parent, cargo
/// reporting Fresh while `CARGO_MANIFEST_DIR` still names the parent.
fn names_the_tree(path: &[u8], source: &Path) -> bool {
    let path = Path::new(std::ffi::OsStr::from_bytes(path));
    if path.is_relative() {
        return true;
    }
    path.starts_with(source) && !path.starts_with(source.join("target"))
}

/// The hash ending a unit's name. A fingerprint directory spells the package and a dep-info file the
/// crate, which need not match, so only this part names one unit across the two.
fn hash_of(name: &str) -> Option<&str> {
    let hash = name.rsplit('-').next()?;
    (hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hash)
}

fn package_of(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |(package, _)| package)
}

/// Why nothing was carried, in the row the model reads; a rebuild follows, slower and never wrong.
#[derive(Debug, PartialEq, Eq)]
pub enum Carried {
    /// No cargo project here, no `target/` at the source, or the child built already.
    Nothing,
    Units(usize),
    Refused(String),
}

/// The hashes of every unit a package in this tree built, by the dep-info under `profile`: a
/// run-build-script unit has no dep-info, so one bad unit drops its whole package. `Err` past a cap.
fn in_tree_units(profile: &Path, source: &Path) -> Result<HashSet<String>, String> {
    let mut bad = HashSet::new();
    let mut seen = 0usize;
    let build = std::fs::read_dir(profile.join("build"))
        .into_iter()
        .flatten()
        .flatten();
    let dirs = std::iter::once(profile.join("deps")).chain(build.map(|entry| entry.path()));
    for dir in dirs {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "d") {
                continue;
            }
            seen = seen.saturating_add(1);
            if seen > DEP_INFO_FILES_MAX {
                return Err(format!(
                    "[cargo units not carried: past {DEP_INFO_FILES_MAX} dep-info files (fork.depinfo_files {DEP_INFO_FILES_MAX}); the child rebuilds them]"
                ));
            }
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|file| {
                file.take(DEP_INFO_MAX_BYTES.saturating_add(1))
                    .read_to_end(&mut bytes)
            });
            let unread = read.is_err() || bytes.len() as u64 > DEP_INFO_MAX_BYTES;
            if unread
                || dep_info(&bytes)
                    .iter()
                    .any(|dep| names_the_tree(dep, source))
            {
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned());
                bad.extend(stem.as_deref().and_then(hash_of).map(str::to_owned));
            }
        }
    }
    let units: Vec<String> = std::fs::read_dir(profile.join(".fingerprint"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let packages: HashSet<&str> = (units.iter())
        .filter(|name| hash_of(name).is_some_and(|hash| bad.contains(hash)))
        .map(|name| package_of(name))
        .collect();
    let same_package = units
        .iter()
        .filter(|name| packages.contains(package_of(name)));
    bad.extend(same_package.filter_map(|name| hash_of(name).map(str::to_owned)));
    Ok(bad)
}

/// Into `target.tmp-<id>`, `.fingerprint` first so a racing parent reads stale rather than fresh, then
/// renamed whole. A unit whose fingerprint directory is gone rebuilds, so a partial carry is safe.
pub fn carry_cargo(source: &Path, tree: &Path, id: &str) -> Carried {
    let profile = source.join("target/debug");
    if !tree.join("Cargo.lock").is_file() || tree.join("target").exists() || !profile.is_dir() {
        return Carried::Nothing;
    }
    let dropped = match in_tree_units(&profile, source) {
        Ok(dropped) => dropped,
        Err(row) => return Carried::Refused(row),
    };
    let temp = temp_of(tree, id);
    let _a_crash_left_it = std::fs::remove_dir_all(&temp);
    let mut carried = 0usize;
    let into = temp.join("debug");
    for dir in [".fingerprint", "build", "deps"] {
        let to = into.join(dir);
        if std::fs::create_dir_all(&to).is_err() {
            return Carried::Refused(
                "[cargo units not carried: no target directory; the child rebuilds them]"
                    .to_owned(),
            );
        }
        for entry in std::fs::read_dir(profile.join(dir))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name();
            let stem = Path::new(&name)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned());
            let unit = stem.as_deref().and_then(hash_of);
            if unit.is_none_or(|hash| dropped.contains(hash)) {
                continue;
            }
            if copy_unit(&entry.path(), &to.join(&name)).is_ok() {
                carried = carried.saturating_add(1);
            }
        }
    }
    match std::fs::rename(&temp, tree.join("target")) {
        Ok(()) => Carried::Units(carried),
        Err(error) => Carried::Refused(format!(
            "[cargo units not carried: {error}; the child rebuilds them]"
        )),
    }
}

pub(super) fn temp_of(tree: &Path, id: &str) -> PathBuf {
    tree.join(format!("target.tmp-{id}"))
}

#[cfg(target_os = "macos")]
fn copy_unit(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, CloneFlags, Mode, OFlags, fclonefileat, openat};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let source = openat(CWD, from, flags, Mode::empty())?;
    fclonefileat(&source, CWD, to, CloneFlags::NOOWNERCOPY).map_err(Into::into)
}

/// Ext4 has no clone. Only artifacts cargo replaces whole hardlink, so an honest build never writes
/// through to the parent's; a hostile child still can, the disk trade the trust table names.
#[cfg(not(target_os = "macos"))]
fn copy_unit(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_unit(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    let name = from
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let replaced_whole = name.starts_with("build-script-")
        || [".rlib", ".rmeta", ".so", ".a"]
            .iter()
            .any(|ext| name.ends_with(ext));
    if replaced_whole && std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(drop)
}

/// `.worktreeinclude`: gitignore-style lines, a `#` comment or a blank skipped. The patterns kept
/// and how many it listed, or why none of it was read.
pub fn include_list(bytes: &[u8]) -> Result<(Vec<String>, usize), String> {
    if bytes.len() > INCLUDE_LIST_MAX_BYTES {
        return Err(format!(
            "{} bytes, past fork.include_list {INCLUDE_LIST_MAX_BYTES}",
            bytes.len()
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_owned())?;
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let listed = lines.clone().count();
    let patterns = lines
        .take(INCLUDE_PATTERNS_MAX)
        .map(str::to_owned)
        .collect();
    Ok((patterns, listed))
}

/// One bad pattern is named and the rest still match.
fn matcher(patterns: &[String], rows: &mut Vec<String>) -> Option<globset::GlobSet> {
    let mut set = globset::GlobSetBuilder::new();
    for pattern in patterns {
        let trimmed = pattern.trim_start_matches('/').trim_end_matches('/');
        if trimmed.starts_with('!') {
            rows.push(format!(
                "[not carried: pattern `{}` is a negation, which this list has no rule order for; list the files it names]",
                crate::ext::sanitize(pattern)
            ));
            continue;
        }
        match globset::Glob::new(trimmed) {
            Ok(glob) => drop(set.add(glob)),
            Err(error) => rows.push(format!(
                "[not carried: pattern `{}` does not parse: {}]",
                crate::ext::sanitize(pattern),
                error.kind()
            )),
        }
    }
    set.build().ok()
}

/// A path copied, or why not: the walk refuses a link on any component, so a path a contained
/// parent swapped copies nothing.
fn include_one(
    parent: &Path,
    tree: &Path,
    rel: &Path,
    denied: &[PathBuf],
    bytes: &mut u64,
) -> Result<(), String> {
    use rustix::fs::{CWD, Mode, OFlags, fstat, openat};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let mut dir = openat(CWD, parent, flags | OFlags::DIRECTORY, Mode::empty())
        .map_err(|error| error.to_string())?;
    let parts: Vec<_> = rel
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let Some((leaf, ancestors)) = parts.split_last() else {
        return Err("an empty path".to_owned());
    };
    for part in ancestors {
        dir = openat(
            &dir,
            part.as_os_str(),
            flags | OFlags::DIRECTORY,
            Mode::empty(),
        )
        .map_err(|error| format!("a link on the way: {error}"))?;
    }
    let file = openat(&dir, leaf.as_os_str(), flags, Mode::empty())
        .map_err(|error| format!("a link at the end: {error}"))?;
    let stat = fstat(&file).map_err(|error| error.to_string())?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
        return Err("not a regular file".to_owned());
    }
    let real = parent.join(rel);
    if denied.iter().any(|store| real.starts_with(store)) {
        return Err("a wall or a credential store denies it".to_owned());
    }
    *bytes = bytes.saturating_add(u64::try_from(stat.st_size).unwrap_or(u64::MAX));
    if *bytes > INCLUDE_BYTES_MAX {
        return Err(format!("past fork.include_bytes {INCLUDE_BYTES_MAX}"));
    }
    let to = tree.join(rel);
    if let Some(up) = to.parent() {
        std::fs::create_dir_all(up).map_err(|error| error.to_string())?;
    }
    let mut from = std::fs::File::from(file);
    let mut out = std::fs::File::create(to).map_err(|error| error.to_string())?;
    std::io::copy(&mut from, &mut out)
        .map(drop)
        .map_err(|error| error.to_string())
}

/// `.worktreeinclude` and a child's `carry=` over the ignored paths: a file must be both ignored and
/// listed. Files only: status collapses `target/` and `.venv/` to a directory, which refuses by name.
pub fn carry_includes(
    parent: &Path,
    tree: &Path,
    ignored: &[String],
    extra: &[String],
    denied: &[PathBuf],
) -> Vec<String> {
    let mut rows = Vec::new();
    let mut patterns = Vec::new();
    if let Ok(bytes) = std::fs::read(parent.join(".worktreeinclude")) {
        match include_list(&bytes) {
            Ok((kept, listed)) => {
                if kept.len() < listed {
                    rows.push(yi_tools::cut_row(
                        kept.len(),
                        listed,
                        "patterns read from .worktreeinclude",
                        "fork.include_patterns",
                        INCLUDE_PATTERNS_MAX,
                        "carry=[...] on rlm.run",
                    ));
                }
                patterns = kept;
            }
            Err(why) => rows.push(format!(
                "[.worktreeinclude not read: {why}; shorten it, or name the files with carry=[...] on rlm.run]"
            )),
        }
    }
    patterns.extend(extra.iter().cloned());
    if patterns.is_empty() {
        return rows;
    }
    let Some(set) = matcher(&patterns, &mut rows) else {
        return rows;
    };
    let (mut copied, mut bytes) = (0usize, 0u64);
    let matched = (ignored.iter())
        .map(|path| path.trim_end_matches('/'))
        .filter(|path| set.is_match(path));
    for path in matched {
        if copied >= INCLUDE_FILES_MAX {
            rows.push(yi_tools::cut_row(
                copied,
                copied.saturating_add(1),
                "listed files copied",
                "fork.include_files",
                INCLUDE_FILES_MAX,
                "copy the rest by hand from the parent",
            ));
            break;
        }
        match include_one(parent, tree, Path::new(path), denied, &mut bytes) {
            Ok(()) => copied = copied.saturating_add(1),
            Err(why) => rows.push(format!("[not carried: {path}: {why}]")),
        }
    }
    if copied > 0 {
        rows.push(format!("carried {copied} ignored files into the child"));
    }
    rows
}
