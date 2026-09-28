//! `program` (plan section 5.4): one kernel cell's source, frozen as an artifact, journaled, and
//! appended to `program.py`. An audit export: nothing here or anywhere else executes it.

use std::io::Write;

use yi_types::plan::canonical::{ArtifactRef, Digest};
use yi_types::plan::doc::{PlanId, TouchCount};
use yi_types::plan::ledger::{JournalRecord, RequestId};
use yi_types::plan::op::CellId;

use super::ops::{Actor, Op, Outcome, PlanEngine, PlanOpError};
use super::state::root_of;
use super::table::op_name;

pub const PROGRAM_NAME: &str = "program.py";

pub(crate) fn iso(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (year, month, day) = yi_kernel::client::civil_from_days(seconds / 86_400);
    let (hour, minute, second) = (
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn header(cell: &str) -> String {
    format!("# --- cell {cell} ")
}

/// What one cell adds to the export; a superseded plan's first cell opens its version.
fn section(existing: &str, version: u64, cell: &str, at: u64, source: &str) -> String {
    let marker = format!("# --- version {version}\n");
    let mut out = String::new();
    if version > 1 && !existing.contains(&marker) {
        out.push_str(&marker);
    }
    out.push_str(&format!("{}{}\n{source}", header(cell), iso(at)));
    if !source.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn refused(detail: impl Into<String>) -> PlanOpError {
    PlanOpError::Program {
        detail: detail.into(),
    }
}

/// The cell a committed `program` record of `plan` names, with its source digest.
fn recorded(record: &JournalRecord, plan: &PlanId) -> Option<(String, Digest)> {
    if record.record.op != op_name(super::table::OpKind::Program)
        || &record.record.plan != plan
        || record.is_refusal()
    {
        return None;
    }
    let cell = record.args.get("cell_id")?.as_str()?.to_owned();
    let source = record.args.get("source_ref")?.clone();
    let source: ArtifactRef = serde_json::from_value(source).ok()?;
    Some((cell, source.digest))
}

impl PlanEngine {
    fn program_path(&self, id: &PlanId) -> std::path::PathBuf {
        self.store().plan_dir(id).join(PROGRAM_NAME)
    }

    fn program_text(&self, id: &PlanId) -> Result<String, PlanOpError> {
        match std::fs::read_to_string(self.program_path(id)) {
            Ok(text) => Ok(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(refused(format!("{PROGRAM_NAME}: {error}"))),
        }
    }

    fn source_of(&self, id: &PlanId, digest: &Digest) -> Result<String, PlanOpError> {
        let bytes = self
            .store()
            .artifacts(id)
            .get(digest)
            .map_err(|error| refused(format!("the source is not in the store: {error}")))?;
        String::from_utf8(bytes).map_err(|_| refused("the source artifact is not UTF-8 text"))
    }

    /// The sha256 `program.py` will have once this cell is appended; the record carries it.
    pub(super) fn program_hash(
        &self,
        id: &PlanId,
        version: u64,
        cell: &CellId,
        source: &ArtifactRef,
        at: u64,
    ) -> Result<Digest, PlanOpError> {
        let mut text = self.program_text(id)?;
        let source = self.source_of(id, &source.digest)?;
        text.push_str(&section(&text, version, cell.as_str(), at, &source));
        Ok(Digest::of(text.as_bytes()))
    }

    /// Appends every journaled cell past those the file holds, walked over each cell's own bytes:
    /// a crash between commit and append heals without a rewrite, and a quoted marker is source.
    fn export(
        &self,
        id: &PlanId,
        version: u64,
        records: &[JournalRecord],
    ) -> Result<(), PlanOpError> {
        let mut text = self.program_text(id)?;
        let (mut walked, mut added) = (0, String::new());
        for record in records {
            let Some((cell, digest)) = recorded(record, id) else {
                continue;
            };
            let source = self.source_of(id, &digest)?;
            let rest = text.get(walked..).unwrap_or_default();
            let rest = rest
                .strip_prefix("# --- version ")
                .and_then(|marked| marked.split_once('\n'))
                .map_or(rest, |(_, after)| after);
            let bare = section("", 1, &cell, record.record.at, &source);
            if added.is_empty()
                && let Some(after) = rest.strip_prefix(bare.as_str())
            {
                walked = text.len().saturating_sub(after.len());
                continue;
            }
            let part = section(&text, version, &cell, record.record.at, &source);
            text.push_str(&part);
            added.push_str(&part);
        }
        if added.is_empty() {
            return Ok(());
        }
        let path = self.program_path(id);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                file.write_all(added.as_bytes())?;
                file.sync_all()
            })
            .map_err(|error| refused(format!("{}: {error}", path.display())))
    }

    /// Invariant: the source is read back from the store before the record names it, and a
    /// cell id is recorded once: the export is keyed on it.
    pub(super) fn program(
        &self,
        plan: Option<PlanId>,
        actor: &Actor,
        op: Op,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let Op::Program {
            cell_id,
            source_ref,
        } = &op
        else {
            return Err(PlanOpError::NotJournaled { op: op.kind() });
        };
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let mut txn = self.begin(&root, actor, request, expected, false)?;
        let version = txn.state.plan(&id)?.version.0;
        // Invariant: the export is healed before the record is built, so `program_hash` names
        // the file this cell is appended to and not one a crash left a cell short.
        self.export(&id, version, &txn.records)?;
        if let Some(replayed) = self.replay(&txn, &op)? {
            return Ok(replayed);
        }
        self.check_revision(&txn, &id, expected)?;
        self.source_of(&id, &source_ref.digest)?;
        let again = txn
            .records
            .iter()
            .filter_map(|record| recorded(record, &id))
            .any(|(cell, _)| cell == cell_id.as_str());
        if again {
            return Err(refused(format!("cell {cell_id} is already recorded")));
        }
        let outcome = self.settle(&mut txn, &id, &root, &op)?;
        self.export(&id, version, &txn.records)?;
        Ok(outcome)
    }
}
