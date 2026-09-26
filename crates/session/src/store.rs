use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::record::LaneRecord;
use yi_types::wire::{Fact, Mutation};

use crate::error::SessionError;
use crate::id::{IdGenerator, now_ms};
use crate::query::{
    BranchBounds, EntryOrder, EntryQuery, ForkScope, HistoryHit, LanePointer, LogOptions,
    RecordQuery, SessionMetadata, SessionStats,
};
use crate::state::SessionState;

pub struct SessionStore {
    metadata: SessionMetadata,
    state: SessionState,
    file: Option<PathBuf>,
    ids: IdGenerator,
}

fn encode_line(mutation: &Mutation) -> Result<String, SessionError> {
    serde_json::to_string(mutation)
        .map(|json| format!("{json}\n"))
        .map_err(|error| {
            SessionError::Storage(format!("Failed to encode session mutation: {error}"))
        })
}

impl SessionStore {
    pub fn in_memory(metadata: SessionMetadata) -> Self {
        Self {
            metadata,
            state: SessionState::new(),
            file: None,
            ids: IdGenerator::new(),
        }
    }

    pub fn file_backed(metadata: SessionMetadata, path: PathBuf) -> Self {
        Self {
            metadata,
            state: SessionState::new(),
            file: Some(path),
            ids: IdGenerator::new(),
        }
    }

    pub fn metadata(&self) -> &SessionMetadata {
        &self.metadata
    }

    pub fn file_path(&self) -> Option<&PathBuf> {
        self.file.as_ref()
    }

    pub(crate) fn replay(&mut self, mutation: Mutation) -> Result<(), SessionError> {
        self.state.apply_mutation(mutation)?;
        self.sync_name();
        Ok(())
    }

    /// Mirrors a replayed `Fact::Name` into the metadata snapshot every caller reads.
    fn sync_name(&mut self) {
        self.metadata.name = self.state.name().map(str::to_owned);
    }

    fn commit(&mut self, mutation: Mutation) -> Result<(), SessionError> {
        if let Some(path) = &self.file {
            let line = encode_line(&mutation)?;
            let mut file = OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|error| {
                    SessionError::Storage(format!(
                        "Failed to append session {}: {error}",
                        path.display()
                    ))
                })?;
            file.write_all(line.as_bytes()).map_err(|error| {
                SessionError::Storage(format!(
                    "Failed to append session {}: {error}",
                    path.display()
                ))
            })?;
        }
        self.state.apply_mutation(mutation)?;
        self.sync_name();
        Ok(())
    }

    pub fn append_entry(&mut self, mut entry: Entry, lane: &str) -> Result<Entry, SessionError> {
        let parent_id = self.state.require_lane(lane)?;
        self.state.validate_unused_id(entry.id())?;
        entry.assign(parent_id, self.state.next_sequence(), now_ms());
        self.commit(Mutation::Entry {
            lane: Some(lane.to_owned()),
            entry: entry.clone(),
        })?;
        Ok(entry)
    }

    pub fn append_record(&mut self, mut record: LaneRecord) -> Result<LaneRecord, SessionError> {
        self.state.require_lane(record.lane())?;
        self.state.validate_unused_id(record.id())?;
        if matches!(record, LaneRecord::OperationStarted { .. })
            && let Some(open_id) = self.state.open_operation_id(record.lane())
        {
            return Err(SessionError::Storage(format!(
                "Lane {} already has an open operation {open_id}",
                record.lane()
            )));
        }
        record.assign(self.state.next_sequence(), now_ms());
        self.commit(Mutation::Record {
            record: record.clone(),
        })?;
        Ok(record)
    }

    pub fn append_message(
        &mut self,
        lane: &str,
        message: AgentMessage,
    ) -> Result<String, SessionError> {
        let id = self.ids.next_id();
        let entry = Entry::Message {
            id: id.clone(),
            message,
            terminate: None,
            parent_id: None,
            seq: 0,
            timestamp: 0,
        };
        self.append_entry(entry, lane)?;
        Ok(id)
    }

    pub fn next_id(&mut self) -> String {
        self.ids.next_id()
    }

    pub fn append_compaction(
        &mut self,
        lane: &str,
        summary: String,
        retained_tail: Vec<AgentMessage>,
        tokens_before: u64,
        details: Option<Value>,
    ) -> Result<String, SessionError> {
        let id = self.ids.next_id();
        let entry = Entry::Compaction {
            id: id.clone(),
            summary,
            retained_tail,
            tokens_before,
            details,
            usage: None,
            parent_id: None,
            seq: 0,
            timestamp: 0,
        };
        self.append_entry(entry, lane)?;
        Ok(id)
    }

    /// `from_id` is the leaf the lane left, so a reader can tell which attempt the summary stands in for.
    pub fn append_branch_summary(
        &mut self,
        lane: &str,
        from_id: String,
        summary: String,
    ) -> Result<String, SessionError> {
        let id = self.ids.next_id();
        let entry = Entry::BranchSummary {
            id: id.clone(),
            from_id,
            summary,
            details: None,
            usage: None,
            parent_id: None,
            seq: 0,
            timestamp: 0,
        };
        self.append_entry(entry, lane)?;
        Ok(id)
    }

    pub fn append_custom(
        &mut self,
        lane: &str,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionError> {
        let id = self.ids.next_id();
        let entry = Entry::Custom {
            id: id.clone(),
            custom_type: custom_type.to_owned(),
            data,
            parent_id: None,
            seq: 0,
            timestamp: 0,
        };
        self.append_entry(entry, lane)?;
        Ok(id)
    }

    pub fn create_lane(&mut self, lane: &str, at: Option<&str>) -> Result<(), SessionError> {
        self.state.validate_new_lane(lane)?;
        self.state.validate_target(at)?;
        self.commit(Mutation::Lane {
            seq: self.state.next_sequence(),
            lane: lane.to_owned(),
            leaf_id: at.map(str::to_owned),
        })
    }

    pub fn move_lane(&mut self, lane: &str, to: Option<&str>) -> Result<(), SessionError> {
        self.state.require_lane(lane)?;
        self.state.validate_target(to)?;
        self.commit(Mutation::Lane {
            seq: self.state.next_sequence(),
            lane: lane.to_owned(),
            leaf_id: to.map(str::to_owned),
        })
    }

    pub fn set_name(&mut self, name: Option<String>) -> Result<(), SessionError> {
        self.commit(Mutation::Fact {
            seq: self.state.next_sequence(),
            fact: Fact::Name { name },
        })
    }

    pub fn set_goal(&mut self, goal: yi_types::goal::Goal) -> Result<(), SessionError> {
        self.commit(Mutation::Fact {
            seq: self.state.next_sequence(),
            fact: Fact::Goal { goal },
        })
    }

    pub fn goal(&self) -> Option<yi_types::goal::Goal> {
        self.state.goal().cloned()
    }

    pub fn set_plan(&mut self, plan: yi_types::plan::Plan) -> Result<(), SessionError> {
        self.commit(Mutation::Fact {
            seq: self.state.next_sequence(),
            fact: Fact::Plan { plan },
        })
    }

    pub fn plan(&self) -> Option<yi_types::plan::Plan> {
        self.state.plan().cloned()
    }

    pub fn set_label(
        &mut self,
        target_id: &str,
        label: Option<String>,
    ) -> Result<(), SessionError> {
        self.state.validate_target(Some(target_id))?;
        self.commit(Mutation::Fact {
            seq: self.state.next_sequence(),
            fact: Fact::Label {
                target_id: target_id.to_owned(),
                label,
            },
        })
    }

    pub fn lanes(&self) -> Vec<LanePointer> {
        self.state.lanes()
    }

    pub fn leaf_id(&self, lane: &str) -> Result<Option<String>, SessionError> {
        self.state.require_lane(lane)
    }

    pub fn entry(&self, id: &str) -> Option<Entry> {
        self.state.entry(id).cloned()
    }

    pub fn find_entries(&self, query: &EntryQuery) -> Result<Vec<Entry>, SessionError> {
        self.state.find_entries(query)
    }

    pub fn grep(&self, needle: &str, limit: usize) -> Vec<HistoryHit> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, 32);
        let lowered_needle = needle.to_lowercase();
        let query = EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        };
        let Ok(entries) = self.find_entries(&query) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|entry| {
                let lowered = searchable_text(entry).to_lowercase();
                if !lowered.contains(&lowered_needle) {
                    return None;
                }
                Some(HistoryHit {
                    entry_id: entry.id().to_owned(),
                    entry_type: entry.type_name().to_owned(),
                    snippet: snippet(&lowered, &lowered_needle),
                })
            })
            .take(limit)
            .collect()
    }

    pub fn find_entries_on_branch(
        &self,
        lane: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        crate::state::validate_limit(query.limit)?;
        let start = match &bounds.start {
            Some(start) => Some(start.clone()),
            None => self.state.require_lane(lane)?,
        };
        match start {
            None => Ok(Vec::new()),
            Some(start) => self.state.find_entries_on_branch(&start, query, bounds),
        }
    }

    pub fn find_records(&self, query: &RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        self.state.find_records(query)
    }

    pub fn find_open_operations(
        &self,
        lane: &str,
        limit: Option<usize>,
    ) -> Result<Vec<LaneRecord>, SessionError> {
        self.state.find_open_operations(lane, limit)
    }

    pub fn log(&self, options: &LogOptions) -> Result<Vec<Mutation>, SessionError> {
        self.state.log(options)
    }

    pub fn name(&self) -> Option<String> {
        self.state.name().map(str::to_owned)
    }

    pub fn label(&self, id: &str) -> Option<String> {
        self.state.label(id).map(str::to_owned)
    }

    pub fn stats(&self) -> SessionStats {
        self.state.stats()
    }

    pub fn fork_mutations(&self, scope: &ForkScope) -> Result<Vec<Mutation>, SessionError> {
        self.state.fork_mutations(scope)
    }
}

const SNIPPET_CHARS: usize = 160;

fn searchable_text(entry: &Entry) -> String {
    match entry {
        Entry::Message { message, .. } => message.plain_text(),
        Entry::Compaction { summary, .. } | Entry::BranchSummary { summary, .. } => summary.clone(),
        _ => String::new(),
    }
}

fn snippet(lowered: &str, lowered_needle: &str) -> String {
    let line = lowered
        .lines()
        .find(|line| line.contains(lowered_needle))
        .or_else(|| lowered.lines().next())
        .unwrap_or("");
    if line.chars().count() <= SNIPPET_CHARS {
        return line.to_owned();
    }
    let char_pos = line.find(lowered_needle).map_or(0, |byte| {
        line.char_indices().take_while(|(i, _)| *i < byte).count()
    });
    let start = char_pos.saturating_sub(SNIPPET_CHARS / 2);
    line.chars().skip(start).take(SNIPPET_CHARS).collect()
}
