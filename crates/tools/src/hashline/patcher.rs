use std::path::{Path, PathBuf};

use super::apply::apply_edits;
use super::blocks::{brace_block_resolver, indent_block_resolver, resolve_block_edits};
use super::clipboard::{OnEmptyPaste, fork_clipboard, start_clipboard_batch};
use super::format::{
    FileTag, HL_FILE_HASH_LENGTH, HL_FILE_HASH_SEP, compute_file_hash, format_hashline_header,
    split_addressable_file_lines,
};
use super::input::{Patch, PatchSection};
use super::messages::{
    HEADTAIL_DRIFT_WARNING, RefusalRows, anchored_lines, missing_snapshot_tag_message,
    path_recovered_from_tag_message, rebased_warning, refusal_footer, refusal_rows,
    unseen_lines_message,
};
use super::mismatch::mismatch_message;
use super::normalize::{Endings, split};
use super::rebase::{LineMap, remap_edits};
use super::snapshots::{Snapshot, SnapshotStore};
use super::types::{ApplyResult, BlockResolverRequest, BlockSpan, Clipboard, Edit, FileOp};

pub(crate) const SEEN_LINE_REVEAL_CAP: usize = 40;
/// The one clip width for revealed and read rows alike; an over-wide line
/// truncates so no line joins the seen set.
pub(crate) const SEEN_LINE_REVEAL_MAX_COLUMNS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionOp {
    Create,
    Update,
    Delete,
    Noop,
}

#[derive(Debug, Clone)]
pub struct PatchSectionResult {
    pub path: String,
    pub canonical_path: String,
    pub op: SectionOp,
    pub before: String,
    pub after: String,
    pub file_hash: FileTag,
    pub header: String,
    pub first_changed_line: Option<u64>,
    pub warnings: Vec<String>,
    pub move_dest: Option<String>,
}

pub struct PreparedSection {
    section: PatchSection,
    canonical_path: String,
    exists: bool,
    endings: Endings,
    normalized: String,
    apply_result: ApplyResult,
    parse_warnings: Vec<String>,
    file_op: Option<FileOp>,
}

impl PreparedSection {
    pub fn is_noop(&self) -> bool {
        self.file_op.is_none() && self.apply_result.text == self.normalized
    }

    pub fn diff_inputs(&self) -> (&str, &str) {
        (&self.normalized, &self.apply_result.text)
    }

    pub fn path(&self) -> &str {
        &self.section.path
    }
}

pub struct Patcher<'a> {
    pub snapshots: &'a mut SnapshotStore,
    pub cwd: PathBuf,
}

/// Chosen by extension: indentation-scoped languages have no closer to scan for.
pub fn block_resolver(request: &BlockResolverRequest<'_>) -> Option<BlockSpan> {
    let extension = Path::new(request.path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    match extension {
        "py" | "pyi" | "yaml" | "yml" => indent_block_resolver(request.text, request.line),
        "md" | "markdown" => super::blocks::markdown_section_resolver(request.text, request.line),
        _ => brace_block_resolver(request.text, request.line),
    }
}

impl<'a> Patcher<'a> {
    pub fn new(snapshots: &'a mut SnapshotStore, cwd: PathBuf) -> Self {
        Self { snapshots, cwd }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        yi_permission::resolve_target(path, &self.cwd)
    }

    fn canonical_path(&self, path: &str) -> String {
        let resolved = self.resolve_path(path);
        resolved
            .canonicalize()
            .unwrap_or(resolved)
            .to_string_lossy()
            .into_owned()
    }

    /// Preflights every section in memory before any write hits disk, then commits in order;
    /// a mid-batch failure reports which sections landed so only the rest are re-issued.
    pub fn apply(
        &mut self,
        patch: &Patch,
        host_clipboard: &mut Clipboard,
    ) -> Result<Vec<PatchSectionResult>, String> {
        let mut clipboard = start_clipboard_batch(host_clipboard);
        let mut prepared: Vec<PreparedSection> = Vec::new();
        let mut section_states: Vec<Clipboard> = Vec::new();
        for section in &patch.sections {
            prepared.push(self.prepare(section, &mut clipboard)?);
            section_states.push(fork_clipboard(&clipboard));
        }
        let mut seen_canonical: Vec<(&str, &str)> = Vec::new();
        for entry in &prepared {
            if let Some((_, previous)) = seen_canonical
                .iter()
                .find(|(canonical, _)| *canonical == entry.canonical_path)
            {
                return Err(format!(
                    "Multiple hashline sections resolve to the same file ({previous} and {}). Merge their ops under one header before applying.",
                    entry.section.path
                ));
            }
            seen_canonical.push((&entry.canonical_path, &entry.section.path));
        }
        if prepared.len() > 1 || patch.sections.len() > 1 {
            for entry in &prepared {
                if entry.is_noop() {
                    return Err(format!(
                        "Edits to {} resulted in no changes being made.",
                        entry.section.path
                    ));
                }
            }
        }

        let mut results: Vec<PatchSectionResult> = Vec::new();
        for (index, entry) in prepared.iter().enumerate() {
            match self.commit(entry) {
                Ok(result) => results.push(result),
                Err(error) => {
                    let written: Vec<&str> = prepared[..index]
                        .iter()
                        .map(|entry| entry.section.path.as_str())
                        .collect();
                    let not_written: Vec<&str> = prepared[index + 1..]
                        .iter()
                        .map(|entry| entry.section.path.as_str())
                        .collect();
                    let mut message = format!("Failed to write {}: {error}", entry.section.path);
                    if !written.is_empty() {
                        message.push_str(&format!(
                            " Sections already written: {}.",
                            written.join(", ")
                        ));
                    }
                    if !not_written.is_empty() {
                        message.push_str(&format!(
                            " Sections not written: {}.",
                            not_written.join(", ")
                        ));
                    }
                    return Err(message);
                }
            }
            if let Some(state) = section_states.get(index) {
                super::clipboard::commit_clipboard(state.clone(), host_clipboard);
            }
        }
        Ok(results)
    }

    pub fn prepare(
        &mut self,
        section: &PatchSection,
        clipboard: &mut Clipboard,
    ) -> Result<PreparedSection, String> {
        let parsed = section.parse()?;
        let mut parse_warnings = parsed.warnings.clone();
        let file_op = parsed.file_op.clone();

        let mut target = section.clone();
        let mut canonical_path = self.canonical_path(&target.path);
        let mut read = self.try_read(&target.path);
        // No tag names the version this session last showed for the path.
        let expected = match section.file_hash.as_deref() {
            Some(text) => FileTag::parse(text).ok_or_else(|| {
                format!("Tag {HL_FILE_HASH_SEP}{text} is not {HL_FILE_HASH_LENGTH} hex digits.")
            })?,
            None => match self.snapshots.head(&canonical_path) {
                Some(head) => head.hash,
                None => {
                    return Err(match read.as_deref() {
                        Some(text) => self.missing_snapshot(section, &canonical_path, text)?,
                        None => missing_snapshot_tag_message(&section.path, None),
                    });
                }
            },
        };

        // Path recovery: the authored path doesn't exist, but its filename plus snapshot tag
        // may name a file read this session — a bare filename or wrong dir. Rebind and warn.
        if read.is_none() {
            let authored_name = Path::new(&target.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut candidates: Vec<String> = self
                .snapshots
                .find_by_hash(expected)
                .into_iter()
                .filter(|snapshot| {
                    Path::new(&snapshot.path)
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy() == authored_name)
                })
                .map(|snapshot| snapshot.path.clone())
                .collect();
            candidates.sort();
            candidates.dedup();
            candidates.retain(|candidate| self.canonical_path(candidate) != canonical_path);
            if candidates.len() == 1 {
                let resolved = candidates.remove(0);
                parse_warnings.push(path_recovered_from_tag_message(
                    &target.path,
                    &resolved,
                    expected,
                ));
                target = target.with_path(resolved);
                canonical_path = self.canonical_path(&target.path);
                read = self.try_read(&target.path);
            }
        }

        let Some(raw_content) = read else {
            return Err(format!(
                "File not found: {}. Use the write tool to create new files.",
                target.path
            ));
        };

        if let Some(FileOp::Move { dest }) = &file_op
            && self.canonical_path(dest) == canonical_path
        {
            return Err(format!("MV destination is the same as {}.", target.path));
        }

        let (normalized, endings) = split(&raw_content);

        let edits = if matches!(file_op, Some(FileOp::Rem)) {
            Vec::new()
        } else {
            parsed.edits
        };
        let apply_result = self.apply_with_validation(
            &target,
            &canonical_path,
            &normalized,
            edits,
            clipboard,
            expected,
        )?;

        Ok(PreparedSection {
            section: target,
            canonical_path,
            exists: true,
            endings,
            normalized,
            apply_result,
            parse_warnings,
            file_op,
        })
    }

    /// Approval of a diff is not approval of a path: re-validate symlink status at write
    /// time, since writing through one lands the content outside the reviewed target.
    fn refuse_symlink(&self, path: &str) -> Result<(), String> {
        let resolved = self.resolve_path(path);
        if let Ok(metadata) = std::fs::symlink_metadata(&resolved)
            && metadata.file_type().is_symlink()
        {
            return Err(format!(
                "{path} is a symlink; refusing to write through it. Edit the target file directly."
            ));
        }
        Ok(())
    }

    pub fn commit(&mut self, prepared: &PreparedSection) -> Result<PatchSectionResult, String> {
        let section = &prepared.section;
        let after = prepared.apply_result.text.clone();
        let mut warnings = prepared.parse_warnings.clone();
        warnings.extend(prepared.apply_result.warnings.iter().cloned());
        let move_dest = match &prepared.file_op {
            Some(FileOp::Move { dest }) => Some(dest.clone()),
            _ => None,
        };

        if matches!(prepared.file_op, Some(FileOp::Rem)) {
            self.refuse_symlink(&section.path)?;
            std::fs::remove_file(self.resolve_path(&section.path))
                .map_err(|error| format!("failed to delete {}: {error}", section.path))?;
            self.snapshots.invalidate(&prepared.canonical_path);
            let hash = compute_file_hash(&prepared.normalized);
            return Ok(PatchSectionResult {
                path: section.path.clone(),
                canonical_path: prepared.canonical_path.clone(),
                op: SectionOp::Delete,
                before: prepared.normalized.clone(),
                after: prepared.normalized.clone(),
                file_hash: hash,
                header: format_hashline_header(&section.path, hash),
                first_changed_line: None,
                warnings,
                move_dest: None,
            });
        }

        if after == prepared.normalized && move_dest.is_none() {
            let hash = self
                .snapshots
                .record(&prepared.canonical_path, &prepared.normalized, None);
            return Ok(PatchSectionResult {
                path: section.path.clone(),
                canonical_path: prepared.canonical_path.clone(),
                op: SectionOp::Noop,
                before: prepared.normalized.clone(),
                after: prepared.normalized.clone(),
                file_hash: hash,
                header: format_hashline_header(&section.path, hash),
                first_changed_line: None,
                warnings,
                move_dest: None,
            });
        }

        let persisted = prepared
            .endings
            .restore(&after, &prepared.apply_result.origins);

        if let Some(dest) = move_dest {
            self.refuse_symlink(&section.path)?;
            let dest_canonical = self.canonical_path(&dest);
            self.snapshots
                .relocate(&prepared.canonical_path, &dest_canonical);
            let dest_resolved = self.resolve_path(&dest);
            if let Some(parent) = dest_resolved.parent() {
                let _best_effort = std::fs::create_dir_all(parent);
            }
            std::fs::write(&dest_resolved, &persisted)
                .map_err(|error| format!("failed to write {dest}: {error}"))?;
            std::fs::remove_file(self.resolve_path(&section.path))
                .map_err(|error| format!("failed to remove {}: {error}", section.path))?;
            let file_hash = self.snapshots.record(&dest_canonical, &after, None);
            return Ok(PatchSectionResult {
                path: dest.clone(),
                canonical_path: dest_canonical,
                op: SectionOp::Update,
                before: prepared.normalized.clone(),
                after,
                file_hash,
                header: format_hashline_header(&dest, file_hash),
                first_changed_line: prepared.apply_result.first_changed_line,
                warnings,
                move_dest: Some(dest),
            });
        }

        self.refuse_symlink(&section.path)?;
        std::fs::write(self.resolve_path(&section.path), &persisted)
            .map_err(|error| format!("failed to write {}: {error}", section.path))?;
        let file_hash = self
            .snapshots
            .record(&prepared.canonical_path, &after, None);
        Ok(PatchSectionResult {
            path: section.path.clone(),
            canonical_path: prepared.canonical_path.clone(),
            op: if prepared.exists {
                SectionOp::Update
            } else {
                SectionOp::Create
            },
            before: prepared.normalized.clone(),
            after,
            file_hash,
            header: format_hashline_header(&section.path, file_hash),
            first_changed_line: prepared.apply_result.first_changed_line,
            warnings,
            move_dest: None,
        })
    }

    /// A path never shown, or a tag no snapshot backs: mint the live tag, show, refuse.
    fn missing_snapshot(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        text: &str,
    ) -> Result<String, String> {
        let (normalized, _) = split(text);
        let (anchors, total, shown) = section_rows(section, &normalized)?;
        let tag = self
            .snapshots
            .record(canonical_path, &normalized, Some(&shown.seen));
        let footer = refusal_footer(&section.path, tag, &shown, &anchors, total);
        Ok(missing_snapshot_tag_message(
            &section.path,
            Some((tag, &shown.rows, &footer)),
        ))
    }

    fn try_read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.resolve_path(path)).ok()
    }

    fn assert_seen_lines(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        normalized: &str,
    ) -> Result<(), String> {
        // A live-hash tag with no snapshot behind it (evicted, prior session, collision): closed.
        let Some(snapshot) = self
            .snapshots
            .by_content(canonical_path, normalized)
            .cloned()
        else {
            return Err(self.missing_snapshot(section, canonical_path, normalized)?);
        };
        self.assert_seen_lines_in(section, canonical_path, &snapshot)
    }

    fn assert_seen_lines_in(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        snapshot: &Snapshot,
    ) -> Result<(), String> {
        let Some(seen) = &snapshot.seen_lines else {
            return Ok(());
        };
        // Incident: a `read` with `limit: 0` recorded an empty set, read here as unrestricted.
        if seen.is_empty() && snapshot.text.is_empty() {
            return Ok(());
        }
        let lines = split_addressable_file_lines(&snapshot.text);
        let total = u64::try_from(lines.len()).unwrap_or(u64::MAX);
        let unseen: Vec<u64> = section
            .collect_anchor_lines()?
            .into_iter()
            .filter(|line| *line <= total && !seen.contains(line))
            .collect();
        if unseen.is_empty() {
            return Ok(());
        }
        let shown = refusal_rows(&anchored_lines(&unseen, total), &unseen, &lines);
        self.snapshots
            .record_seen_lines(canonical_path, snapshot.hash, &shown.seen);
        let footer = refusal_footer(&section.path, snapshot.hash, &shown, &unseen, total);
        Err(unseen_lines_message(
            &section.path,
            &unseen,
            snapshot.hash,
            &shown.rows,
            &footer,
        ))
    }

    /// Anchors from an older version of the file move through the diff onto the current
    /// text, or the section is rejected with the current lines under the cited numbers.
    fn rebase_edits(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        normalized: &str,
        edits: Vec<Edit>,
        expected: FileTag,
        old: &Snapshot,
    ) -> Result<Vec<Edit>, String> {
        self.assert_seen_lines_in(section, canonical_path, old)?;
        let map = LineMap::between(&old.text, normalized);
        let old_block = |line: u64| {
            block_resolver(&BlockResolverRequest {
                path: &section.path,
                text: &old.text,
                line,
            })
        };
        match remap_edits(edits, &map, &old_block) {
            Ok(edits) => Ok(edits),
            Err(_unmapped) => {
                Err(self.mismatch_error(section, canonical_path, normalized, expected, true)?)
            }
        }
    }

    fn apply_with_validation(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        normalized: &str,
        edits: Vec<Edit>,
        clipboard: &mut Clipboard,
        expected: FileTag,
    ) -> Result<ApplyResult, String> {
        let live_hash = compute_file_hash(normalized);
        let mut warnings: Vec<String> = Vec::new();

        let edits = if expected == live_hash {
            self.assert_seen_lines(section, canonical_path, normalized)?;
            edits
        } else {
            let Some(old) = self.snapshots.by_hash(canonical_path, expected).cloned() else {
                return Err(self.mismatch_error(
                    section,
                    canonical_path,
                    normalized,
                    expected,
                    false,
                )?);
            };
            if !section.collect_anchor_lines()?.is_empty() {
                let edits =
                    self.rebase_edits(section, canonical_path, normalized, edits, expected, &old)?;
                warnings.push(rebased_warning(old.hash, live_hash));
                edits
            } else {
                // Head/tail-only inserts are position-stable: a stale tag is non-fatal for them.
                warnings.push(HEADTAIL_DRIFT_WARNING.to_owned());
                edits
            }
        };

        let has_blocks = edits.iter().any(|edit| matches!(edit, Edit::Block { .. }));
        let (resolved, mut resolve_warnings) = if has_blocks {
            let resolved =
                resolve_block_edits(edits, &section.path, normalized, Some(&block_resolver))?;
            (resolved.edits, resolved.warnings)
        } else {
            (edits, Vec::new())
        };
        let mut result = apply_edits(normalized, resolved, clipboard, OnEmptyPaste::Throw)?;
        warnings.append(&mut resolve_warnings);
        warnings.append(&mut result.warnings);
        result.warnings = warnings;
        Ok(result)
    }

    fn mismatch_error(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        normalized: &str,
        expected: FileTag,
        hash_recognized: bool,
    ) -> Result<String, String> {
        let (anchors, total, shown) = section_rows(section, normalized)?;
        let actual = self
            .snapshots
            .record(canonical_path, normalized, Some(&shown.seen));
        let footer = refusal_footer(&section.path, actual, &shown, &anchors, total);
        Ok(mismatch_message(
            &section.path,
            &expected.to_string(),
            actual,
            hash_recognized,
            &shown.rows,
            &footer,
        ))
    }
}

/// The anchors, the addressable line count, and the rows a refusal prints (anchors ±2).
fn section_rows(
    section: &PatchSection,
    normalized: &str,
) -> Result<(Vec<u64>, u64, RefusalRows), String> {
    let lines = split_addressable_file_lines(normalized);
    let total = u64::try_from(lines.len()).unwrap_or(u64::MAX);
    let anchors = section.collect_anchor_lines()?;
    let (head, tail) = section.inserts_at_ends()?;
    let mut display = anchors.clone();
    display.extend(head.then_some(1));
    display.extend(tail.then_some(total));
    let shown = refusal_rows(&anchored_lines(&display, total), &anchors, &lines);
    Ok((anchors, total, shown))
}
