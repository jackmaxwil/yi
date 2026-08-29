use std::collections::{HashMap, HashSet};

use yi_types::entry::Entry;
use yi_types::record::LaneRecord;
use yi_types::wire::{Fact, Mutation};

use crate::error::SessionError;
use crate::query::{
    BranchBounds, EntryOrder, EntryQuery, ForkPosition, ForkScope, LanePointer, LogOptions,
    RecordQuery, SessionStats,
};

pub(crate) fn validate_limit(limit: Option<usize>) -> Result<(), SessionError> {
    match limit {
        Some(0) => Err(SessionError::InvalidQuery(
            "limit must be a positive integer".to_owned(),
        )),
        _ => Ok(()),
    }
}

pub struct SessionState {
    sequence: u64,
    used_ids: HashSet<String>,
    entries: Vec<Entry>,
    entries_by_id: HashMap<String, usize>,
    records: Vec<LaneRecord>,
    open_ops_by_lane: HashMap<String, Vec<usize>>,
    lanes: Vec<(String, Option<String>)>,
    log: Vec<Mutation>,
    stats: SessionStats,
    name: Option<String>,
    labels: HashMap<String, String>,
    goal: Option<yi_types::goal::Goal>,
    plan: Option<yi_types::plan::Plan>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            sequence: 0,
            used_ids: HashSet::new(),
            entries: Vec::new(),
            entries_by_id: HashMap::new(),
            records: Vec::new(),
            open_ops_by_lane: HashMap::new(),
            lanes: vec![("main".to_owned(), None)],
            log: Vec::new(),
            stats: SessionStats::zero(),
            name: None,
            goal: None,
            plan: None,
            labels: HashMap::new(),
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.saturating_add(1)
    }

    pub fn lanes(&self) -> Vec<LanePointer> {
        self.lanes
            .iter()
            .map(|(lane, leaf_id)| LanePointer {
                lane: lane.clone(),
                leaf_id: leaf_id.clone(),
            })
            .collect()
    }

    fn lane_leaf(&self, lane: &str) -> Option<&Option<String>> {
        self.lanes
            .iter()
            .find(|(name, _)| name == lane)
            .map(|(_, leaf)| leaf)
    }

    pub fn require_lane(&self, lane: &str) -> Result<Option<String>, SessionError> {
        self.lane_leaf(lane)
            .cloned()
            .ok_or_else(|| SessionError::InvalidLane(lane.to_owned()))
    }

    pub fn validate_new_lane(&self, lane: &str) -> Result<(), SessionError> {
        if self.lane_leaf(lane).is_some() {
            return Err(SessionError::AlreadyExists(format!(
                "Lane already exists: {lane}"
            )));
        }
        Ok(())
    }

    pub fn validate_target(&self, target_id: Option<&str>) -> Result<(), SessionError> {
        match target_id {
            Some(id) if !self.entries_by_id.contains_key(id) => {
                Err(SessionError::NotFound(format!("Entry not found: {id}")))
            }
            _ => Ok(()),
        }
    }

    pub fn validate_unused_id(&self, id: &str) -> Result<(), SessionError> {
        if self.used_ids.contains(id) {
            return Err(SessionError::AlreadyExists(format!(
                "Session id already exists: {id}"
            )));
        }
        Ok(())
    }

    pub fn open_operation_id(&self, lane: &str) -> Option<&str> {
        self.open_ops_by_lane
            .get(lane)
            .and_then(|indices| indices.last())
            .and_then(|index| self.records.get(*index))
            .map(LaneRecord::id)
    }

    /// Main-lane tokens live on assistant entries; child lanes arrive as
    /// Usage records. Counting only records reported ~zero for the main lane.
    fn absorb_entry_usage(&mut self, entry: &Entry) {
        if let Entry::Message { message, .. } = entry
            && let yi_types::message::AgentMessage::Assistant { usage, .. } = message
        {
            self.stats.cached_tokens = self.stats.cached_tokens.saturating_add(usage.cache_read);
            self.stats.uncached_tokens = self
                .stats
                .uncached_tokens
                .saturating_add(usage.input.saturating_add(usage.cache_write));
            self.stats.total_tokens = self.stats.total_tokens.saturating_add(usage.total_tokens);
            self.stats.cost_total += usage.cost.total.as_f64().unwrap_or(0.0);
        }
    }

    pub fn apply_mutation(&mut self, mutation: Mutation) -> Result<(), SessionError> {
        let invalid = |message: String| {
            SessionError::InvalidEntry(format!("Invalid session mutation: {message}"))
        };
        let seq = match &mutation {
            Mutation::Entry { entry, .. } => entry.seq(),
            Mutation::Record { record } => record.seq(),
            Mutation::Lane { seq, .. } | Mutation::Fact { seq, .. } => *seq,
        };
        if seq != self.next_sequence() {
            return Err(invalid(format!("has non-consecutive seq {seq}")));
        }
        match &mutation {
            Mutation::Entry { lane, entry } => {
                if self.used_ids.contains(entry.id()) {
                    return Err(invalid(format!("contains duplicate id {}", entry.id())));
                }
                if let Some(lane) = lane {
                    let leaf = self
                        .lane_leaf(lane)
                        .ok_or_else(|| invalid(format!("references missing lane {lane}")))?;
                    if entry.parent_id() != leaf.as_deref() {
                        return Err(invalid("does not chain to the lane leaf".to_owned()));
                    }
                }
                if let Some(parent) = entry.parent_id()
                    && !self.entries_by_id.contains_key(parent)
                {
                    return Err(invalid(format!("references missing parent {parent}")));
                }
                self.sequence = seq;
                self.used_ids.insert(entry.id().to_owned());
                self.entries_by_id
                    .insert(entry.id().to_owned(), self.entries.len());
                self.entries.push(entry.clone());
                if let Some(lane) = lane
                    && let Some(slot) = self.lanes.iter_mut().find(|(name, _)| name == lane)
                {
                    slot.1 = Some(entry.id().to_owned());
                }
                if matches!(entry, Entry::Message { .. }) {
                    self.stats.message_count = self.stats.message_count.saturating_add(1);
                }
                self.absorb_entry_usage(entry);
            }
            Mutation::Record { record } => {
                if self.lane_leaf(record.lane()).is_none() {
                    return Err(invalid(format!(
                        "references missing lane {}",
                        record.lane()
                    )));
                }
                if self.used_ids.contains(record.id()) {
                    return Err(invalid(format!("contains duplicate id {}", record.id())));
                }
                self.sequence = seq;
                self.used_ids.insert(record.id().to_owned());
                let index = self.records.len();
                self.records.push(record.clone());
                match record {
                    LaneRecord::OperationStarted { .. } => {
                        self.open_ops_by_lane
                            .entry(record.lane().to_owned())
                            .or_default()
                            .push(index);
                    }
                    LaneRecord::OperationFinished { run_id, .. } => {
                        if let Some(open) = self.open_ops_by_lane.get_mut(record.lane()) {
                            open.retain(|open_index| {
                                self.records
                                    .get(*open_index)
                                    .is_none_or(|started| started.id() != run_id)
                            });
                        }
                    }
                    _ => {}
                }
                // A `cause: "assistant"` usage record mirrors an assistant
                // entry that already carries the same usage (pi import
                // convention); counting both doubles the main lane.
                let mirrors_entry = matches!(
                    record,
                    LaneRecord::Usage { cause, .. } if cause == "assistant"
                );
                if let Some(usage) = record.usage().filter(|_| !mirrors_entry) {
                    self.stats.cached_tokens =
                        self.stats.cached_tokens.saturating_add(usage.cache_read);
                    self.stats.uncached_tokens = self
                        .stats
                        .uncached_tokens
                        .saturating_add(usage.input.saturating_add(usage.cache_write));
                    self.stats.total_tokens =
                        self.stats.total_tokens.saturating_add(usage.total_tokens);
                    self.stats.cost_total += usage.cost.total.as_f64().unwrap_or(0.0);
                }
            }
            Mutation::Lane { lane, leaf_id, .. } => {
                if let Some(leaf) = leaf_id
                    && !self.entries_by_id.contains_key(leaf)
                {
                    return Err(invalid(format!("references missing lane target {leaf}")));
                }
                self.sequence = seq;
                match self.lanes.iter_mut().find(|(name, _)| name == lane) {
                    Some(slot) => slot.1 = leaf_id.clone(),
                    None => self.lanes.push((lane.clone(), leaf_id.clone())),
                }
            }
            Mutation::Fact { fact, .. } => match fact {
                Fact::Name { name } => {
                    self.sequence = seq;
                    self.name = name.clone();
                }
                Fact::Goal { goal } => {
                    self.sequence = seq;
                    self.goal = Some(goal.clone());
                }
                Fact::Plan { plan } => {
                    self.sequence = seq;
                    self.plan = Some(plan.clone());
                }
                Fact::Label { target_id, label } => {
                    if !self.entries_by_id.contains_key(target_id) {
                        return Err(invalid(format!(
                            "references missing label target {target_id}"
                        )));
                    }
                    self.sequence = seq;
                    match label {
                        Some(label) => {
                            self.labels.insert(target_id.clone(), label.clone());
                        }
                        None => {
                            self.labels.remove(target_id);
                        }
                    }
                }
            },
        }
        self.log.push(mutation);
        Ok(())
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries_by_id
            .get(id)
            .and_then(|index| self.entries.get(*index))
    }

    pub fn find_entries(&self, query: &EntryQuery) -> Result<Vec<Entry>, SessionError> {
        validate_limit(query.limit)?;
        let mut results = Vec::new();
        let iter: Box<dyn Iterator<Item = &Entry>> = match query.order {
            EntryOrder::OldestFirst => Box::new(self.entries.iter()),
            EntryOrder::NewestFirst => Box::new(self.entries.iter().rev()),
        };
        for entry in iter {
            if !matches_entry_query(entry, query) {
                continue;
            }
            results.push(entry.clone());
            if Some(results.len()) == query.limit {
                break;
            }
        }
        Ok(results)
    }

    pub fn find_entries_on_branch(
        &self,
        start: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        validate_limit(query.limit)?;
        let mut results = Vec::new();
        match query.order {
            EntryOrder::OldestFirst => {
                let path = self.walk_to_root(start, &BranchBounds::default())?;
                for entry in path.iter().rev() {
                    let reached_bound = Some(entry.id()) == bounds.stop_at_id.as_deref()
                        || Some(entry.type_name()) == bounds.stop_at_type;
                    if matches_entry_query(entry, query) {
                        results.push((*entry).clone());
                    }
                    if reached_bound || Some(results.len()) == query.limit {
                        break;
                    }
                }
            }
            EntryOrder::NewestFirst => {
                for entry in self.walk_to_root(start, bounds)? {
                    if matches_entry_query(entry, query) {
                        results.push(entry.clone());
                    }
                    if Some(results.len()) == query.limit {
                        break;
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn find_records(&self, query: &RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        validate_limit(query.limit)?;
        if query.operation_kind.is_some() && query.record_type != Some("operation_started") {
            return Err(SessionError::InvalidQuery(
                "operationKind requires type \"operation_started\"".to_owned(),
            ));
        }
        let mut results = Vec::new();
        let iter: Box<dyn Iterator<Item = &LaneRecord>> = match query.order {
            EntryOrder::OldestFirst => Box::new(self.records.iter()),
            EntryOrder::NewestFirst => Box::new(self.records.iter().rev()),
        };
        for record in iter {
            if !matches_record_query(record, query) {
                continue;
            }
            results.push(record.clone());
            if Some(results.len()) == query.limit {
                break;
            }
        }
        Ok(results)
    }

    pub fn find_open_operations(
        &self,
        lane: &str,
        limit: Option<usize>,
    ) -> Result<Vec<LaneRecord>, SessionError> {
        validate_limit(limit)?;
        let open = self
            .open_ops_by_lane
            .get(lane)
            .map(|indices| {
                indices
                    .iter()
                    .rev()
                    .filter_map(|index| self.records.get(*index).cloned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(match limit {
            Some(limit) => open.into_iter().take(limit).collect(),
            None => open,
        })
    }

    pub fn log(&self, options: &LogOptions) -> Result<Vec<Mutation>, SessionError> {
        validate_limit(options.limit)?;
        let mut results = Vec::new();
        for item in &self.log {
            let seq = match item {
                Mutation::Entry { entry, .. } => entry.seq(),
                Mutation::Record { record } => record.seq(),
                Mutation::Lane { seq, .. } | Mutation::Fact { seq, .. } => *seq,
            };
            if let Some(after) = options.after_seq
                && seq <= after
            {
                continue;
            }
            results.push(item.clone());
            if Some(results.len()) == options.limit {
                break;
            }
        }
        Ok(results)
    }

    pub fn plan(&self) -> Option<&yi_types::plan::Plan> {
        self.plan.as_ref()
    }

    pub fn goal(&self) -> Option<&yi_types::goal::Goal> {
        self.goal.as_ref()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn label(&self, id: &str) -> Option<&str> {
        self.labels.get(id).map(String::as_str)
    }

    pub fn stats(&self) -> SessionStats {
        self.stats.clone()
    }

    pub fn fork_mutations(&self, scope: &ForkScope) -> Result<Vec<Mutation>, SessionError> {
        let (copied, fork_lanes) = match scope {
            ForkScope::Tree => (
                self.find_entries(&EntryQuery {
                    order: EntryOrder::OldestFirst,
                    ..EntryQuery::default()
                })?,
                self.lanes(),
            ),
            ForkScope::Branch { entry_id, position } => {
                let selected = match entry_id {
                    Some(id) => Some(id.clone()),
                    None => self.require_lane("main")?,
                };
                let target_id = match selected {
                    None => None,
                    Some(selected) => {
                        let entry = self
                            .entry(&selected)
                            .filter(|entry| matches!(entry, Entry::Message { .. }));
                        let Some(entry) = entry else {
                            return Err(SessionError::InvalidForkTarget(format!(
                                "Fork target is not a message entry: {selected}"
                            )));
                        };
                        let position = position.unwrap_or(if entry_id.is_none() {
                            ForkPosition::At
                        } else {
                            ForkPosition::Before
                        });
                        match position {
                            ForkPosition::At => Some(entry.id().to_owned()),
                            ForkPosition::Before => entry.parent_id().map(str::to_owned),
                        }
                    }
                };
                let copied = match &target_id {
                    None => Vec::new(),
                    Some(start) => self.find_entries_on_branch(
                        start,
                        &EntryQuery {
                            order: EntryOrder::OldestFirst,
                            ..EntryQuery::default()
                        },
                        &BranchBounds::default(),
                    )?,
                };
                (
                    copied,
                    vec![LanePointer {
                        lane: "main".to_owned(),
                        leaf_id: target_id,
                    }],
                )
            }
        };

        let mut mutations = Vec::new();
        let mut sequence: u64 = 1;
        for source in &copied {
            let mut entry = source.clone();
            entry.set_seq(sequence);
            sequence = sequence.saturating_add(1);
            mutations.push(Mutation::Entry { lane: None, entry });
        }
        for pointer in fork_lanes {
            mutations.push(Mutation::Lane {
                seq: sequence,
                lane: pointer.lane,
                leaf_id: pointer.leaf_id,
            });
            sequence = sequence.saturating_add(1);
        }
        if let Some(name) = &self.name {
            mutations.push(Mutation::Fact {
                seq: sequence,
                fact: Fact::Name {
                    name: Some(name.clone()),
                },
            });
            sequence = sequence.saturating_add(1);
        }
        for entry in &copied {
            if let Some(label) = self.labels.get(entry.id()) {
                mutations.push(Mutation::Fact {
                    seq: sequence,
                    fact: Fact::Label {
                        target_id: entry.id().to_owned(),
                        label: Some(label.clone()),
                    },
                });
                sequence = sequence.saturating_add(1);
            }
        }
        Ok(mutations)
    }

    fn walk_to_root<'a>(
        &'a self,
        start: &str,
        bounds: &BranchBounds,
    ) -> Result<Vec<&'a Entry>, SessionError> {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        let mut current = self
            .entry(start)
            .ok_or_else(|| SessionError::NotFound(format!("Entry not found: {start}")))?;
        loop {
            if !visited.insert(current.id().to_owned()) {
                return Err(SessionError::InvalidEntry(format!(
                    "Session branch contains a cycle at {}",
                    current.id()
                )));
            }
            path.push(current);
            let at_bound = Some(current.id()) == bounds.stop_at_id.as_deref()
                || Some(current.type_name()) == bounds.stop_at_type;
            let Some(parent_id) = current.parent_id() else {
                break;
            };
            if at_bound {
                break;
            }
            current = self.entry(parent_id).ok_or_else(|| {
                SessionError::InvalidEntry(format!("Entry not found: {parent_id}"))
            })?;
        }
        Ok(path)
    }
}

fn matches_entry_query(entry: &Entry, query: &EntryQuery) -> bool {
    (query.entry_type.is_none() || Some(entry.type_name()) == query.entry_type)
        && (query.custom_type.is_none() || entry.custom_type() == query.custom_type.as_deref())
        && query.after_seq.is_none_or(|after| match query.order {
            EntryOrder::OldestFirst => entry.seq() > after,
            EntryOrder::NewestFirst => entry.seq() < after,
        })
}

fn matches_record_query(record: &LaneRecord, query: &RecordQuery) -> bool {
    (query.lane.is_none() || Some(record.lane()) == query.lane.as_deref())
        && (query.record_type.is_none() || Some(record.type_name()) == query.record_type)
        && query.run_id.as_deref().is_none_or(|run_id| {
            if matches!(record, LaneRecord::OperationStarted { .. }) {
                record.id() == run_id
            } else {
                record.run_id() == Some(run_id)
            }
        })
        && (query.operation_kind.is_none() || record.operation_kind() == query.operation_kind)
        && query.after_seq.is_none_or(|after| record.seq() > after)
}
