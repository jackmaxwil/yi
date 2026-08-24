use std::collections::HashSet;

use super::format::{FileTag, compute_file_hash};

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub path: String,
    pub text: String,
    pub hash: FileTag,
    pub recorded_at: u64,
    pub seen_lines: Option<HashSet<u64>>,
}

fn merge_seen_lines(snapshot: &mut Snapshot, lines: Option<&[u64]>) {
    let Some(lines) = lines else { return };
    let set = snapshot.seen_lines.get_or_insert_with(HashSet::new);
    set.extend(lines.iter().copied());
}

// Wide sessions routinely touch far more than a few dozen files; evicting a
// path downgrades a genuinely in-session tag to the misleading "hash is not
// from this session" rejection. Retention is still bounded by MAX_TOTAL_BYTES.
const MAX_PATHS: usize = 256;
const MAX_VERSIONS_PER_PATH: usize = 4;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// In-memory snapshot store: per-path history is a short ring of full-file
/// versions, path tracking is LRU-bounded (front = most recent). Two distinct
/// texts colliding on the 16-bit tag are retained as separate versions — the
/// tag is a fast index, never the identity.
#[derive(Default)]
pub struct SnapshotStore {
    paths: Vec<(String, Vec<Snapshot>)>,
    clock: u64,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn touch(&mut self, path: &str) -> Option<&mut Vec<Snapshot>> {
        let position = self.paths.iter().position(|(name, _)| name == path)?;
        let entry = self.paths.remove(position);
        self.paths.insert(0, entry);
        self.paths.first_mut().map(|(_, history)| history)
    }

    fn total_bytes(&self) -> usize {
        self.paths
            .iter()
            .flat_map(|(_, history)| history.iter())
            .map(|snapshot| snapshot.text.len())
            .sum()
    }

    fn evict(&mut self) {
        while self.paths.len() > MAX_PATHS
            || (self.total_bytes() > MAX_TOTAL_BYTES && self.paths.len() > 1)
        {
            self.paths.pop();
        }
    }

    pub fn head(&self, path: &str) -> Option<&Snapshot> {
        self.paths
            .iter()
            .find(|(name, _)| name == path)
            .and_then(|(_, history)| history.first())
    }

    /// When two distinct texts collide on the 16-bit tag, the most recent wins.
    pub fn by_hash(&self, path: &str, hash: FileTag) -> Option<&Snapshot> {
        self.paths
            .iter()
            .find(|(name, _)| name == path)
            .and_then(|(_, history)| history.iter().find(|version| version.hash == hash))
    }

    pub fn by_content(&self, path: &str, full_text: &str) -> Option<&Snapshot> {
        self.paths
            .iter()
            .find(|(name, _)| name == path)
            .and_then(|(_, history)| history.iter().find(|version| version.text == full_text))
    }

    /// Every retained version whose tag equals `hash`, across all paths —
    /// tag-based path recovery for a section naming a path that does not exist.
    pub fn find_by_hash(&self, hash: FileTag) -> Vec<&Snapshot> {
        self.paths
            .iter()
            .flat_map(|(_, history)| history.iter())
            .filter(|version| version.hash == hash)
            .collect()
    }

    pub fn record(&mut self, path: &str, full_text: &str, seen_lines: Option<&[u64]>) -> FileTag {
        let hash = compute_file_hash(full_text);
        let recorded_at = self.tick();
        if self.touch(path).is_none() {
            self.paths.insert(0, (path.to_owned(), Vec::new()));
        }
        let Some((_, history)) = self.paths.first_mut() else {
            return hash;
        };
        // Dedup requires full-text equality, not just tag equality: two distinct
        // texts sharing the 4-hex tag are DIFFERENT snapshots — fusing them
        // would corrupt seen-lines and let the patcher misresolve which
        // snapshot the section tag names (omp issue #4075).
        if let Some(position) = history
            .iter()
            .position(|version| version.hash == hash && version.text == full_text)
        {
            let mut existing = history.remove(position);
            existing.recorded_at = recorded_at;
            merge_seen_lines(&mut existing, seen_lines);
            history.insert(0, existing);
            return hash;
        }
        let mut snapshot = Snapshot {
            path: path.to_owned(),
            text: full_text.to_owned(),
            hash,
            recorded_at,
            seen_lines: None,
        };
        merge_seen_lines(&mut snapshot, seen_lines);
        history.insert(0, snapshot);
        history.truncate(MAX_VERSIONS_PER_PATH);
        self.evict();
        hash
    }

    pub fn record_seen_lines(&mut self, path: &str, hash: FileTag, lines: &[u64]) {
        if let Some(history) = self
            .paths
            .iter_mut()
            .find(|(name, _)| name == path)
            .map(|(_, history)| history)
            && let Some(version) = history.iter_mut().find(|snapshot| snapshot.hash == hash)
        {
            merge_seen_lines(version, Some(lines));
        }
    }

    pub fn invalidate(&mut self, path: &str) {
        self.paths.retain(|(name, _)| name != path);
    }

    /// Move retained history from `from` to `to` so tags minted from reads of
    /// the source path stay valid at the destination after a file move.
    pub fn relocate(&mut self, from: &str, to: &str) {
        let Some(position) = self.paths.iter().position(|(name, _)| name == from) else {
            return;
        };
        let (_, source_history) = self.paths.remove(position);
        if source_history.is_empty() {
            return;
        }
        let relocated: Vec<Snapshot> = source_history
            .into_iter()
            .map(|version| Snapshot {
                path: to.to_owned(),
                ..version
            })
            .collect();
        match self.paths.iter_mut().find(|(name, _)| name == to) {
            None => self.paths.insert(0, (to.to_owned(), relocated)),
            Some((_, dest_history)) => {
                let mut seen: HashSet<FileTag> = HashSet::new();
                let mut merged: Vec<Snapshot> = Vec::new();
                for version in relocated.into_iter().chain(dest_history.drain(..)) {
                    if seen.insert(version.hash) {
                        merged.push(version);
                    }
                }
                merged.truncate(MAX_VERSIONS_PER_PATH);
                *dest_history = merged;
            }
        }
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }
}
