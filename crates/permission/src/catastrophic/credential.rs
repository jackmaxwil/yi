use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{
    CatastrophicContext, FileId, HOME_ROOTS, LINK_HOPS, absolute, beneath_via, denied_file, expand,
    file_id, identities, lexical_normalize, lexically_beneath, resolve_links, stores, with_home,
};
/// Maximum directory entries enumerated per `ReadGate` or command scan.
const READ_SCAN_ENTRY_LIMIT: usize = 100_000;

fn count_scan_entry(entries: &mut usize) -> io::Result<()> {
    if *entries >= READ_SCAN_ENTRY_LIMIT {
        return Err(io::Error::other("credential scan entry limit exceeded"));
    }
    *entries += 1;
    Ok(())
}

fn contains_shell_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?'])
        || (pattern.contains('[') && pattern.contains(']'))
        || (pattern.contains('{') && pattern.contains('}'))
        || ["@(", "+(", "!("].iter().any(|form| pattern.contains(form))
}

/// A linked regular file can name a key from outside its store; ordinary files need no walk.
fn hard_linked_file_in_stores(
    stores: &[PathBuf],
    metadata: Option<&fs::Metadata>,
    snapshot: &mut Option<Option<std::collections::HashSet<FileId>>>,
    scan_entries: &mut usize,
) -> bool {
    let Some(metadata) = metadata.filter(|metadata| is_hard_linked_file(metadata)) else {
        return false;
    };
    let Some(id) = file_id(metadata) else {
        return true;
    };
    match snapshot.get_or_insert_with(|| store_file_ids(stores, scan_entries).ok()) {
        Some(ids) => ids.contains(&id),
        None => true,
    }
}

pub(super) fn gate_hard_linked_file_in_stores(
    stores: &[PathBuf],
    metadata: Option<&fs::Metadata>,
    snapshot: &std::sync::OnceLock<Option<std::collections::HashSet<FileId>>>,
) -> bool {
    let Some(metadata) = metadata.filter(|metadata| is_hard_linked_file(metadata)) else {
        return false;
    };
    let Some(id) = file_id(metadata) else {
        return true;
    };
    match snapshot.get_or_init(|| {
        let mut scan_entries = 0;
        store_file_ids(stores, &mut scan_entries).ok()
    }) {
        Some(ids) => ids.contains(&id),
        None => true,
    }
}

#[cfg(unix)]
fn is_hard_linked_file(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn is_hard_linked_file(_metadata: &fs::Metadata) -> bool {
    false
}

/// Snapshot regular-file identities under credential stores once per gate or command.
/// Directory symlinks are followed, with inode cycle detection; scan errors fail closed.
fn store_file_ids(
    stores: &[PathBuf],
    scan_entries: &mut usize,
) -> io::Result<std::collections::HashSet<FileId>> {
    let mut files = std::collections::HashSet::new();
    let mut pending = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for store in stores {
        let metadata = match fs::metadata(store) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            pending.push(store.clone());
        } else if metadata.is_file() {
            files.insert(file_id(&metadata).ok_or(io::ErrorKind::Unsupported)?);
        }
    }

    while let Some(directory) = pending.pop() {
        let metadata = fs::metadata(&directory)?;
        let directory_id = file_id(&metadata).ok_or(io::ErrorKind::Unsupported)?;
        if !metadata.is_dir() || !visited.insert(directory_id) {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            count_scan_entry(scan_entries)?;
            let path = entry.path();
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.insert(file_id(&metadata).ok_or(io::ErrorKind::Unsupported)?);
            }
        }
    }
    Ok(files)
}

/// A key read into the transcript has already left the machine, so credential stores are
/// read-gated too. Path-shaped arguments only: this reads a command, it does not run one. A path
/// is judged as the read gate judges it, by identity, and a directory in home above a store counts,
/// since a recursive read walks into it, as do the roots homes sit under; a glob counts when it
/// can name a store, a directory above one, or a path inside one (D324). Quotes and backslashes
/// are removed as the shell removes them; a path hidden in a variable or a substitution is not.
pub fn command_reads_credentials(command: &str, context: &CatastrophicContext) -> Option<String> {
    let stores = stores(context);
    let home = context.home_dir.as_deref().map(lexical_normalize);
    let mut above: Vec<PathBuf> = stores
        .iter()
        .flat_map(|store| store.ancestors().skip(1))
        .filter(|dir| home.as_ref().is_some_and(|home| dir.starts_with(home)))
        .chain(HOME_ROOTS.iter().map(Path::new))
        .map(Path::to_path_buf)
        .collect();
    above.sort_unstable();
    above.dedup();

    let arguments: Vec<&str> = command.split_whitespace().skip(1).collect();
    let executable = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let recursive_search = match executable {
        "rg" => true,
        "grep" => arguments.iter().any(|argument| {
            let flag = argument.replace(['\'', '"', '\\'], "");
            flag == "-R"
                || flag == "-r"
                || flag == "--recursive"
                || flag.strip_prefix('-').is_some_and(|flags| {
                    !flags.starts_with('-') && flags.chars().any(|c| c == 'R' || c == 'r')
                })
        }),
        _ => false,
    };
    let implicit_rg_path = executable == "rg" && !rg_has_explicit_path(&arguments);

    let mut store_ids: Option<Vec<FileId>> = None;
    let mut file_ids: Option<Option<std::collections::HashSet<FileId>>> = None;
    let mut scan_entries = 0;
    let mut above_ids: Option<Vec<FileId>> = None;
    let mut real_stores: Option<Vec<PathBuf>> = None;
    for raw in arguments
        .into_iter()
        .take(if implicit_rg_path { 0 } else { usize::MAX })
    {
        let token = raw.replace(['\'', '"', '\\'], "");
        if token.starts_with('-') {
            continue;
        }
        let path = expand(&token, context);
        let has_glob = contains_shell_glob(&token);
        let denied = if has_glob {
            if stores.iter().any(|store| glob_reaches(&path, store)) {
                true
            } else {
                let real_stores = real_stores.get_or_insert_with(|| {
                    stores.iter().map(|store| resolve_links(store)).collect()
                });
                let resolved = resolve_links(&path);
                let reaches_store = [path.as_path(), resolved.as_path()].iter().any(|pattern| {
                    stores
                        .iter()
                        .chain(real_stores.iter())
                        .any(|store| glob_reaches(pattern, store))
                });
                reaches_store
                    || glob_hard_link_alias_in_stores(
                        &resolved,
                        &stores,
                        recursive_search,
                        &mut store_ids,
                        &mut file_ids,
                        &mut scan_entries,
                    )
                    .unwrap_or(true)
            }
        } else if above.binary_search(&path).is_ok() || lexically_beneath(&stores, &path) {
            true
        } else {
            let store_ids = store_ids.get_or_insert_with(|| identities(&stores));
            let above_ids = above_ids.get_or_insert_with(|| identities(&above));
            let opened = resolve_links(&absolute(Path::new(&with_home(&token, context)), context));
            let metadata = fs::metadata(&path).ok();
            beneath_via(&stores, store_ids, &path, LINK_HOPS)
                || beneath_via(&stores, store_ids, &opened, LINK_HOPS)
                || denied_file(above_ids, metadata.as_ref())
                || hard_linked_file_in_stores(
                    &stores,
                    metadata.as_ref(),
                    &mut file_ids,
                    &mut scan_entries,
                )
                || (recursive_search
                    && metadata.as_ref().is_some_and(fs::Metadata::is_dir)
                    && recursive_path_has_credentials(
                        &path,
                        &stores,
                        store_ids,
                        &mut file_ids,
                        &mut scan_entries,
                    )
                    .unwrap_or(true))
        };
        if denied {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    if implicit_rg_path {
        let Some(path) = context.working_dir.as_deref() else {
            return Some(command.to_owned());
        };
        let path = lexical_normalize(path);
        if above.binary_search(&path).is_ok() || lexically_beneath(&stores, &path) {
            return Some(path.to_string_lossy().into_owned());
        }
        let store_ids = store_ids.get_or_insert_with(|| identities(&stores));
        if recursive_path_has_credentials(
            &path,
            &stores,
            store_ids,
            &mut file_ids,
            &mut scan_entries,
        )
        .unwrap_or(true)
        {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    None
}

fn rg_has_explicit_path(arguments: &[&str]) -> bool {
    let mut pattern_seen = false;
    let mut explicit_path = false;
    let mut option_value = false;
    let mut after_options = false;
    for raw in arguments {
        let token = raw.replace(['\'', '"', '\\'], "");
        if option_value {
            option_value = false;
            continue;
        }
        if !after_options && token == "--" {
            after_options = true;
            continue;
        }
        if !after_options && token.starts_with('-') {
            match token.as_str() {
                "-e" | "--regexp" | "-f" | "--file" => {
                    pattern_seen = true;
                    option_value = true;
                }
                "-g" | "--glob" | "--iglob" | "-t" | "--type" | "-T" | "--type-not"
                | "--type-add" | "--type-clear" | "--max-depth" | "--max-filesize"
                | "--max-columns" | "-A" | "--after-context" | "-B" | "--before-context" | "-C"
                | "--context" | "-m" | "--max-count" | "-j" | "--threads" | "--color"
                | "--engine" | "--encoding" => {
                    option_value = true;
                }
                _ if token.starts_with("--regexp=")
                    || token.starts_with("--file=")
                    || token.starts_with("-e")
                    || token.starts_with("-f") =>
                {
                    pattern_seen = true;
                }
                _ => {}
            }
            continue;
        }
        if pattern_seen {
            explicit_path = true;
        } else {
            pattern_seen = true;
        }
    }
    explicit_path
}

fn metadata_if_present(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Expands concrete glob components and tests each path the shell would pass to the command.
fn glob_hard_link_alias_in_stores(
    pattern: &Path,
    stores: &[PathBuf],
    recursive: bool,
    store_ids: &mut Option<Vec<FileId>>,
    file_ids: &mut Option<Option<std::collections::HashSet<FileId>>>,
    scan_entries: &mut usize,
) -> io::Result<bool> {
    let mut paths = vec![if pattern.is_absolute() {
        PathBuf::from("/")
    } else {
        PathBuf::from(".")
    }];
    for component in pattern.components() {
        let name = match component {
            Component::Prefix(prefix) => {
                paths = vec![PathBuf::from(prefix.as_os_str())];
                continue;
            }
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => {
                paths.iter_mut().for_each(|path| path.push(".."));
                continue;
            }
            Component::Normal(name) => name,
        };
        let glob = name.to_string_lossy();
        if glob == "**" {
            paths = expand_globstar(paths, scan_entries)?;
            continue;
        }
        let has_glob = contains_shell_glob(&glob);
        if !has_glob {
            paths.iter_mut().for_each(|path| path.push(name));
            continue;
        }

        let glob = globset_pattern(&glob)?;
        let matcher = globset::GlobBuilder::new(&glob)
            .literal_separator(true)
            .case_insensitive(true)
            .backslash_escape(false)
            .build()
            .map_err(|_| io::ErrorKind::InvalidInput)?
            .compile_matcher();
        let dotted = matches!(glob.chars().next(), Some('.' | '{' | '[' | '@'));
        let mut matches = Vec::new();
        for directory in paths {
            let Some(metadata) = metadata_if_present(&directory)? else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                count_scan_entry(scan_entries)?;
                let file_name = entry.file_name();
                let text = file_name.to_string_lossy();
                if (dotted || !text.starts_with('.')) && matcher.is_match(text.as_ref()) {
                    matches.push(entry.path());
                }
            }
        }
        paths = matches;
        if paths.is_empty() {
            return Ok(false);
        }
    }

    for path in paths {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let root_ids = store_ids.get_or_insert_with(|| identities(stores));
        if path_is_in_stores(stores, root_ids, &path, &metadata)
            || hard_linked_file_in_stores(stores, Some(&metadata), file_ids, scan_entries)
            || (recursive
                && metadata.is_dir()
                && recursive_path_has_credentials(&path, stores, root_ids, file_ids, scan_entries)?)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A shell globstar component matches zero or more visible path components. Follow directory
/// links while expanding it, but visit each directory identity only once to stop cycles.
fn expand_globstar(paths: Vec<PathBuf>, scan_entries: &mut usize) -> io::Result<Vec<PathBuf>> {
    let mut matches = paths.clone();
    let mut pending = paths;
    let mut visited = std::collections::HashSet::new();
    while let Some(directory) = pending.pop() {
        let Some(metadata) = metadata_if_present(&directory)? else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let directory_id = file_id(&metadata).ok_or(io::ErrorKind::Unsupported)?;
        if !visited.insert(directory_id) {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            count_scan_entry(scan_entries)?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let child = entry.path();
            let metadata = match fs::metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            matches.push(child.clone());
            if metadata.is_dir() {
                pending.push(child);
            }
        }
    }
    Ok(matches)
}

/// Convert simple `@(a|b)` components to globset's `{a,b}` alternatives. Other extglob
/// operators and nested forms are rejected so callers fail closed instead of missing a match.
fn globset_pattern(glob: &str) -> io::Result<String> {
    if ["?(", "+(", "*(", "!("]
        .iter()
        .any(|form| glob.contains(form))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut output = String::with_capacity(glob.len());
    let mut rest = glob;
    while let Some((prefix, after_open)) = rest.split_once("@(") {
        output.push_str(prefix);
        let (alternatives, suffix) = after_open
            .split_once(')')
            .ok_or(io::ErrorKind::InvalidInput)?;
        if alternatives.contains(['(', ')']) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        output.push('{');
        output.push_str(&alternatives.replace('|', ","));
        output.push('}');
        rest = suffix;
    }
    output.push_str(rest);
    Ok(output)
}

/// Whether a shell glob can match `store`, a directory above it, or a path inside it: compared
/// component by component over the shorter of the two, ignoring letter case, a leading dot
/// matched only by what can open with a dot, as the shell matches it. A resolved pattern keeps its glob text.
fn glob_reaches(pattern: &Path, store: &Path) -> bool {
    let words = |path: &Path| -> Vec<Vec<char>> {
        (path.components())
            .map(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .to_lowercase()
                    .chars()
                    .collect()
            })
            .collect()
    };
    (words(pattern).iter().zip(&words(store))).all(|(glob, name)| {
        // A `{…}` group or a `[…]` class may open with the dot a plain glob never matches.
        let dotted = matches!(glob.first(), Some('.' | '{' | '['));
        let hidden = name.first() == Some(&'.') && !dotted;
        !hidden && wildcard(glob, name)
    })
}

/// `*` any run, `?` and a `[…]` class one character, a `{…}` group any run: a glob that can
/// match more than the shell would only asks more.
fn wildcard(glob: &[char], name: &[char]) -> bool {
    let rest = |after: usize| glob.get(after..).unwrap_or_default();
    let any_run = |after: usize| {
        (0..=name.len()).any(|at| wildcard(rest(after), name.get(at..).unwrap_or_default()))
    };
    match glob.first() {
        None => name.is_empty(),
        Some('*') => any_run(1),
        Some('{') => glob
            .iter()
            .position(|c| *c == '}')
            .is_some_and(|end| any_run(end + 1)),
        Some(first) => {
            let width = match first {
                '?' => 1,
                '[' => glob.iter().position(|c| *c == ']').map_or(1, |end| end + 1),
                _ => 1,
            };
            let same = matches!(first, '?' | '[') || name.first() == Some(first);
            same && !name.is_empty() && wildcard(rest(width), name.get(1..).unwrap_or_default())
        }
    }
}

fn path_is_in_stores(
    stores: &[PathBuf],
    store_ids: &[FileId],
    path: &Path,
    metadata: &fs::Metadata,
) -> bool {
    lexically_beneath(stores, path)
        || denied_file(store_ids, Some(metadata))
        || beneath_via(stores, store_ids, path, LINK_HOPS)
}

/// What a recursive grep can reach, following directory symlinks with identity cycle detection.
fn recursive_path_has_credentials(
    path: &Path,
    stores: &[PathBuf],
    store_ids: &[FileId],
    file_ids: &mut Option<Option<std::collections::HashSet<FileId>>>,
    scan_entries: &mut usize,
) -> io::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if path_is_in_stores(stores, store_ids, path, &metadata)
        || hard_linked_file_in_stores(stores, Some(&metadata), file_ids, scan_entries)
    {
        return Ok(true);
    }
    if !metadata.is_dir() {
        return Ok(false);
    }

    let mut pending = vec![path.to_path_buf()];
    let mut visited = std::collections::HashSet::new();
    while let Some(directory) = pending.pop() {
        let metadata = fs::metadata(&directory)?;
        if !metadata.is_dir() {
            continue;
        }
        let directory_id = file_id(&metadata).ok_or(io::ErrorKind::Unsupported)?;
        if !visited.insert(directory_id) {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            count_scan_entry(scan_entries)?;
            let child = entry.path();
            let metadata = match fs::metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if path_is_in_stores(stores, store_ids, &child, &metadata)
                || hard_linked_file_in_stores(stores, Some(&metadata), file_ids, scan_entries)
            {
                return Ok(true);
            }
            if metadata.is_dir() {
                pending.push(child);
            }
        }
    }
    Ok(false)
}
#[cfg(test)]
mod scan_limit_tests {
    use super::{READ_SCAN_ENTRY_LIMIT, count_scan_entry};

    #[test]
    fn credential_scan_cap_allows_limit_and_rejects_the_next_entry() -> Result<(), std::io::Error> {
        let mut entries = 0;
        for _ in 0..READ_SCAN_ENTRY_LIMIT {
            count_scan_entry(&mut entries)?;
        }
        assert_eq!(entries, READ_SCAN_ENTRY_LIMIT);
        assert_eq!(
            count_scan_entry(&mut entries)
                .err()
                .map(|error| error.kind()),
            Some(std::io::ErrorKind::Other)
        );
        Ok(())
    }
}
