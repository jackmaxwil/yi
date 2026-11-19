//! A server's workspace edit: its files, its text applied, shown as a unified diff, and
//! written to disk atomically.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lsp::protocol::{byte_of, path_of, position};

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("the fix names a file outside the working folder")]
    Outside,
    #[error("the fix creates, renames or deletes a file, which this app does not apply")]
    FileOp,
    #[error("the fix does not have the protocol's shape")]
    Shape,
    #[error("two changes of the fix overlap")]
    Overlap,
    #[error("the file changed since the fix was offered")]
    Stale,
    #[error("a file of the fix could not be read or written: {0}")]
    Io(#[from] io::Error),
}

/// Each file `edit` changes, inside `root`, with its text edits in the order given.
pub fn files<'a>(
    root: &Path,
    edit: &'a Value,
) -> Result<Vec<(PathBuf, Vec<&'a Value>)>, EditError> {
    let mut out: Vec<(PathBuf, Vec<&Value>)> = Vec::new();
    let mut add = |uri: &Value, edits: &'a Value| -> Result<(), EditError> {
        let path = path_of(uri.as_str().ok_or(EditError::Shape)?).ok_or(EditError::Shape)?;
        let path = path.canonicalize()?;
        if !path.starts_with(root) {
            return Err(EditError::Outside);
        }
        let edits = edits.as_array().ok_or(EditError::Shape)?;
        match out.iter_mut().find(|(at, _)| *at == path) {
            Some((_, list)) => list.extend(edits),
            None => out.push((path, edits.iter().collect())),
        }
        Ok(())
    };
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if change.get("kind").is_some() {
                return Err(EditError::FileOp);
            }
            let uri = change
                .pointer("/textDocument/uri")
                .ok_or(EditError::Shape)?;
            add(uri, change.get("edits").ok_or(EditError::Shape)?)?;
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            add(&Value::String(uri.clone()), edits)?;
        }
    }
    Ok(out)
}

/// `text` with `edits` applied; edits at one place keep their order.
pub fn applied(text: &str, edits: &[&Value]) -> Result<String, EditError> {
    let mut spans = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = edit.get("range").ok_or(EditError::Shape)?;
        let start = offset(text, range.get("start")).ok_or(EditError::Shape)?;
        let end = offset(text, range.get("end")).ok_or(EditError::Shape)?;
        let new = edit
            .get("newText")
            .and_then(Value::as_str)
            .ok_or(EditError::Shape)?;
        spans.push((start, end, new));
    }
    spans.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, new) in spans {
        if start < at || end < start {
            return Err(EditError::Overlap);
        }
        out.push_str(text.get(at..start).ok_or(EditError::Shape)?);
        out.push_str(new);
        at = end;
    }
    out.push_str(text.get(at..).ok_or(EditError::Shape)?);
    Ok(out)
}

/// The byte offset of a protocol position in `text`; a line past the end is the end.
fn offset(text: &str, at: Option<&Value>) -> Option<usize> {
    let (line, unit) = position(at?)?;
    let mut start = 0usize;
    for (n, piece) in text.split_inclusive('\n').enumerate() {
        if u64::try_from(n).ok()? == line {
            let body = piece.strip_suffix('\n').unwrap_or(piece);
            let body = body.strip_suffix('\r').unwrap_or(body);
            return start.checked_add(byte_of(body, unit));
        }
        start = start.checked_add(piece.len())?;
    }
    Some(text.len())
}

/// One hunk from the first changed line to the last, with no context lines.
pub fn diff(name: &str, old: &str, new: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()).saturating_sub(head);
    let tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(most)
        .take_while(|(x, y)| x == y)
        .count();
    let gone = a
        .get(head..a.len().saturating_sub(tail))
        .unwrap_or_default();
    let came = b
        .get(head..b.len().saturating_sub(tail))
        .unwrap_or_default();
    let from = |n: usize| if n == 0 { head } else { head.saturating_add(1) };
    let mut out = format!(
        "--- a/{name}\n+++ b/{name}\n@@ -{},{} +{},{} @@\n",
        from(gone.len()),
        gone.len(),
        from(came.len()),
        came.len()
    );
    for line in gone {
        out.push_str(&format!("-{line}\n"));
    }
    for line in came {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

/// Replaces `path` with `text` so a crash leaves the old file or the new, never half of one.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::other("the path names no file"));
    };
    let mut tries = 0u32..16;
    let (tmp, mut file) = loop {
        let Some(n) = tries.next() else {
            return Err(io::Error::other("no free temporary name beside the file"));
        };
        let tmp = dir.join(format!(
            ".{}.yd-{}-{n}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        // Invariant: create_new refuses an existing path or link, so a planted link is never written through.
        match File::options().write(true).create_new(true).open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    };
    let written = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;

    #[test]
    fn edits_on_a_line_with_an_astral_character_land_on_their_characters()
    -> Result<(), Box<dyn Error>> {
        let text = "a\n🦀x = 1;\r\nend";
        let first = json!({"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 3}}, "newText": "_y"});
        let second = json!({"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 0}}, "newText": "!"});
        assert_eq!(applied(text, &[&second, &first])?, "a\n🦀_y = 1;\r\nend!");
        let overlap = json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}, "newText": ""});
        assert!(matches!(
            applied(text, &[&first, &overlap]),
            Err(EditError::Overlap)
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_fix_never_writes_through_a_link_planted_at_the_temporary_name()
    -> Result<(), Box<dyn Error>> {
        let root = crate::fs::tests::scratch("lsp-atomic-link")?;
        let outside = root.join("outside");
        fs::write(&outside, "kept")?;
        let source = root.join("a.rs");
        fs::write(&source, "old")?;
        let planted = root.join(format!(".a.rs.yd-{}-0.tmp", std::process::id()));
        let _ = fs::remove_file(&planted);
        std::os::unix::fs::symlink(&outside, &planted)?;
        write_atomic(&source, "new")?;
        let (kept, got) = (fs::read_to_string(&outside)?, fs::read_to_string(&source)?);
        let linked = fs::symlink_metadata(&source)?.file_type().is_symlink();
        fs::remove_dir_all(&root)?;
        assert_eq!(
            (kept.as_str(), got.as_str(), linked),
            ("kept", "new", false)
        );
        Ok(())
    }

    #[test]
    fn a_diff_shows_only_the_changed_lines() -> Result<(), Box<dyn Error>> {
        let got = diff("a.rs", "one\ntwo\nthree\n", "one\n2\nthree\n");
        assert_eq!(got, "--- a/a.rs\n+++ b/a.rs\n@@ -2,1 +2,1 @@\n-two\n+2\n");
        Ok(())
    }
}
