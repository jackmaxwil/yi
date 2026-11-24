//! Things and addresses: every keyed element can say what it is and where it lives.

pub mod attach;
pub mod thing;
pub mod uri;

/// `path` as a folder `cwd` names it: by its place inside `cwd`, else as given.
pub fn in_folder<'a>(path: &'a str, cwd: &str) -> &'a str {
    let inside = path.strip_prefix(cwd).and_then(|rest| rest.strip_prefix('/'));
    inside.filter(|_| !cwd.is_empty()).unwrap_or(path)
}
