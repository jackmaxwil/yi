use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Transcript, Transcripts};
use crate::wall::Wall;

pub struct SessionTranscripts {
    host: Arc<crate::subagent::SubagentHost>,
    sessions_dir: Option<PathBuf>,
    cwd: String,
    /// Invariant: a history wall names sessions by id, so a name, another spelling of one, is
    /// resolved only for a reader no history wall covers; walled, it fails closed (rule 6).
    names: bool,
}

impl SessionTranscripts {
    pub fn new(
        host: Arc<crate::subagent::SubagentHost>,
        sessions_dir: Option<PathBuf>,
        cwd: &Path,
    ) -> Self {
        Self {
            host,
            sessions_dir,
            cwd: cwd.to_string_lossy().into_owned(),
            names: true,
        }
    }

    /// A root session's file: by its id in any repository, else by its name.
    fn session_file(&self, dir: &Path, agent: &str) -> Option<PathBuf> {
        let cwd = Path::new(&self.cwd);
        let named = || match crate::history::find_named(dir, cwd, agent) {
            crate::history::Named::One(file) => Some(file),
            crate::history::Named::Many(_) | crate::history::Named::Nothing => None,
        };
        crate::history::find_session(dir, cwd, agent).or_else(|| self.names.then(named).flatten())
    }

    pub fn walled_by(mut self, wall: &Wall) -> Self {
        self.names = !wall.denies_history();
        self
    }
}

impl Transcripts for SessionTranscripts {
    fn open(&self, agent: &str) -> Option<Transcript> {
        if let Some(live) = self.host.transcript(agent) {
            return Some(Transcript::Live(live));
        }
        if let Some(kept) = self.host.kept_transcript(agent) {
            return Some(Transcript::Kept(kept));
        }
        if let Some(store) = self
            .host
            .trail(agent)
            .and_then(|file| yi_session::load_session(&file).ok())
        {
            return Some(Transcript::Kept(Arc::new(std::sync::Mutex::new(store))));
        }
        let dir = self.sessions_dir.clone()?;
        let mut repo = yi_session::JsonlRepo::new(dir.clone(), self.cwd.clone());
        if let Ok(session) = yi_session::SessionRepo::open(&mut repo, agent) {
            return Some(Transcript::Kept(session));
        }
        // `root/child` names a child by the session that spawned it: another spelling of the
        // child's own id, so like a name it resolves only past no history wall.
        let child = || {
            let (root, child) = agent.split_once('/')?;
            crate::history::child_of(&self.session_file(&dir, root)?, child)
        };
        let file = (self.session_file(&dir, agent)).or_else(|| self.names.then(child).flatten())?;
        let store = yi_session::load_session(&file).ok()?;
        Some(Transcript::Kept(Arc::new(std::sync::Mutex::new(store))))
    }

    fn candidates(&self, agent: &str) -> Vec<String> {
        let Some(dir) = self.sessions_dir.as_deref().filter(|_| self.names) else {
            return Vec::new();
        };
        match crate::history::find_named(dir, Path::new(&self.cwd), agent) {
            crate::history::Named::Many(files) => files,
            crate::history::Named::One(_) | crate::history::Named::Nothing => Vec::new(),
        }
    }
}
