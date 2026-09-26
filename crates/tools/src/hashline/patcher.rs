use std::path::{Path, PathBuf};

use super::apply::apply_edits;
use super::blocks::{brace_block_resolver, indent_block_resolver, resolve_block_edits};
use super::clipboard::{OnEmptyPaste, fork_clipboard, start_clipboard_batch};
use super::format::{FileTag, compute_file_hash, format_hashline_header};
use super::input::{Patch, PatchSection};
use super::messages::{
    HEADTAIL_DRIFT_WARNING, RevealedLine, UnseenLinesReveal, anchored_lines,
    format_anchored_context, missing_snapshot_tag_message, path_recovered_from_tag_message,
    rebased_warning, unseen_lines_message,
};
use super::mismatch::MismatchError;
use super::normalize::{
    LineEnding, detect_line_ending, normalize_to_lf, restore_line_endings, strip_bom,
};
use super::rebase::{LineMap, remap_edits};
use super::snapshots::{Snapshot, SnapshotStore};
use super::types::{ApplyResult, BlockResolverRequest, BlockSpan, Clipboard, Edit, FileOp};

/// Upper bound on unseen anchor lines revealed inline in a rejection; larger ranges keep the
/// re-read guidance so the model cannot piecewise-reveal its way past the guard.
const SEEN_LINE_REVEAL_CAP: usize = 40;
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
    bom: &'static str,
    line_ending: LineEnding,
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
    pub enforce_seen_lines: bool,
}

fn has_anchor_scoped_edit(edits: &[Edit]) -> bool {
    use super::types::{Cursor, PasteTarget};
    edits.iter().any(|edit| match edit {
        Edit::Delete { .. } | Edit::Block { .. } | Edit::Cut { .. } => true,
        Edit::Paste { at, .. } => match at {
            PasteTarget::Span { .. } => true,
            PasteTarget::Gap { cursor } => {
                matches!(
                    cursor,
                    Cursor::BeforeAnchor { .. } | Cursor::AfterAnchor { .. }
                )
            }
        },
        Edit::Insert { cursor, .. } => {
            matches!(
                cursor,
                Cursor::BeforeAnchor { .. } | Cursor::AfterAnchor { .. }
            )
        }
    })
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
        Self {
            snapshots,
            cwd,
            enforce_seen_lines: true,
        }
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
        let expected_text = match section.file_hash.as_deref() {
            Some(text) => text.to_owned(),
            None => match self.snapshots.head(&canonical_path) {
                Some(head) => head.hash.to_string(),
                None => {
                    return Err(match read.as_deref() {
                        Some(text) => self.missing_snapshot(section, &canonical_path, text)?,
                        None => missing_snapshot_tag_message(&section.path, None),
                    });
                }
            },
        };
        let expected_text = expected_text.as_str();

        // Path recovery: the authored path doesn't exist, but its filename plus snapshot tag
        // may name a file read this session — a bare filename or wrong dir. Rebind and warn.
        if read.is_none()
            && let Some(tag) = FileTag::parse(expected_text)
        {
            let authored_name = Path::new(&target.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut candidates: Vec<String> = self
                .snapshots
                .find_by_hash(tag)
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
                    tag,
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

        let bom_result = strip_bom(&raw_content);
        let bom = bom_result.bom;
        let line_ending = detect_line_ending(bom_result.text);
        let normalized = normalize_to_lf(bom_result.text);

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
            expected_text,
        )?;

        Ok(PreparedSection {
            section: target,
            canonical_path,
            exists: true,
            bom,
            line_ending,
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

        let persisted = format!(
            "{}{}",
            prepared.bom,
            restore_line_endings(&after, prepared.line_ending)
        );

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

    /// A path never shown gets its tag minted from disk and its anchored lines shown (and
    /// marked seen), so the retry needs no read.
    fn missing_snapshot(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        text: &str,
    ) -> Result<String, String> {
        let normalized = normalize_to_lf(text);
        let anchors = section.collect_anchor_lines()?;
        let file_lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
        let shown = anchored_lines(
            &anchors,
            u64::try_from(file_lines.len()).unwrap_or(u64::MAX),
        );
        let tag = self
            .snapshots
            .record(canonical_path, &normalized, Some(&shown));
        let rows = format_anchored_context(&anchors, &file_lines);
        Ok(missing_snapshot_tag_message(
            &section.path,
            Some((tag, &rows)),
        ))
    }

    fn try_read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.resolve_path(path)).ok()
    }

    fn assert_seen_lines(
        &mut self,
        section: &PatchSection,
        normalized: &str,
    ) -> Result<(), String> {
        let canonical = self.canonical_path(&section.path);
        let Some(snapshot) = self.snapshots.by_content(&canonical, normalized).cloned() else {
            return Ok(());
        };
        self.assert_seen_lines_in(section, &snapshot)
    }

    fn assert_seen_lines_in(
        &mut self,
        section: &PatchSection,
        snapshot: &Snapshot,
    ) -> Result<(), String> {
        let Some(seen) = &snapshot.seen_lines else {
            return Ok(());
        };
        // Incident: a `read` with `limit: 0` recorded an empty set, read here as unrestricted.
        if seen.is_empty() && snapshot.text.is_empty() {
            return Ok(());
        }
        let lines = u64::try_from(snapshot.text.split('\n').count()).unwrap_or(u64::MAX);
        let unseen: Vec<u64> = section
            .collect_anchor_lines()?
            .into_iter()
            .filter(|line| *line <= lines && !seen.contains(line))
            .collect();
        if unseen.is_empty() {
            return Ok(());
        }
        let source_lines: Vec<&str> = snapshot.text.split('\n').collect();
        let mut revealed: Vec<RevealedLine> = Vec::new();
        let mut column_truncated = false;
        for &line in unseen.iter().take(SEEN_LINE_REVEAL_CAP) {
            let Some(source) = usize::try_from(line)
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| source_lines.get(index))
            else {
                continue;
            };
            if source.chars().count() > SEEN_LINE_REVEAL_MAX_COLUMNS {
                let clipped: String = source.chars().take(SEEN_LINE_REVEAL_MAX_COLUMNS).collect();
                revealed.push(RevealedLine {
                    line,
                    text: format!("{clipped}\u{2026}"),
                });
                column_truncated = true;
            } else {
                revealed.push(RevealedLine {
                    line,
                    text: (*source).to_owned(),
                });
            }
        }
        let truncated = unseen.len() > revealed.len() || column_truncated;
        // Only merge when the reveal covered every unseen anchor line in full
        // width; a partial reveal must not let a blind edit land piecewise.
        if !truncated {
            let lines: Vec<u64> = revealed.iter().map(|revealed| revealed.line).collect();
            let canonical = self.canonical_path(&section.path);
            self.snapshots
                .record_seen_lines(&canonical, snapshot.hash, &lines);
        }
        Err(unseen_lines_message(
            &section.path,
            &unseen,
            snapshot.hash,
            &UnseenLinesReveal {
                lines: revealed,
                truncated,
            },
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
        expected_text: &str,
        old: &Snapshot,
    ) -> Result<Vec<Edit>, String> {
        if self.enforce_seen_lines {
            self.assert_seen_lines_in(section, old)?;
        }
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
            Err(_unmapped) => Err(self
                .mismatch_error(section, canonical_path, normalized, expected_text, true)?
                .display_message()),
        }
    }

    fn apply_with_validation(
        &mut self,
        section: &PatchSection,
        canonical_path: &str,
        normalized: &str,
        edits: Vec<Edit>,
        clipboard: &mut Clipboard,
        expected_text: &str,
    ) -> Result<ApplyResult, String> {
        let expected = FileTag::parse(expected_text);
        let live_hash = compute_file_hash(normalized);
        let live_matches = expected == Some(live_hash);
        let mut warnings: Vec<String> = Vec::new();

        let edits = if live_matches || expected.is_none() {
            if expected.is_some() && self.enforce_seen_lines {
                self.assert_seen_lines(section, normalized)?;
            }
            edits
        } else if !has_anchor_scoped_edit(&edits) {
            // Head/tail-only inserts are position-stable: a stale tag is non-fatal for them.
            warnings.push(HEADTAIL_DRIFT_WARNING.to_owned());
            edits
        } else {
            let old = expected.and_then(|tag| self.snapshots.by_hash(canonical_path, tag).cloned());
            let Some(old) = old else {
                return Err(self
                    .mismatch_error(section, canonical_path, normalized, expected_text, false)?
                    .display_message());
            };
            let edits = self.rebase_edits(
                section,
                canonical_path,
                normalized,
                edits,
                expected_text,
                &old,
            )?;
            warnings.push(rebased_warning(old.hash, live_hash));
            edits
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
        expected_text: &str,
        hash_recognized: bool,
    ) -> Result<MismatchError, String> {
        let file_lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
        let anchor_lines = section.collect_anchor_lines()?;
        let shown = anchored_lines(
            &anchor_lines,
            u64::try_from(file_lines.len()).unwrap_or(u64::MAX),
        );
        let actual = self
            .snapshots
            .record(canonical_path, normalized, Some(&shown));
        Ok(MismatchError {
            path: Some(section.path.clone()),
            expected_file_hash: expected_text.to_owned(),
            actual_file_hash: actual,
            file_lines,
            anchor_lines,
            hash_recognized,
        })
    }
}
