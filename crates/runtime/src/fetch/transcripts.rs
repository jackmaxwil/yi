use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::url::Url;

use super::{FetchError, Resolver, Transcript, Transcripts};
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
        let file = crate::history::find_session(&dir, Path::new(&self.cwd), agent)?;
        let store = yi_session::load_session(&file).ok()?;
        Some(Transcript::Kept(Arc::new(std::sync::Mutex::new(store))))
    }

    /// `root/child` names a child by the session that spawned it: like a name, another spelling
    /// of the child's own id, so neither resolves past a history wall.
    fn open_named(
        &self,
        path: &str,
    ) -> Result<(Transcript, usize), Vec<crate::history::Candidate>> {
        let dir = (self.sessions_dir.as_deref().filter(|_| self.names)).ok_or_else(Vec::new)?;
        let (top, rest) = path.split_once('/').unwrap_or((path, ""));
        let mut file = match crate::history::find_named(dir, Path::new(&self.cwd), top) {
            crate::history::Named::One(file) => file,
            crate::history::Named::Many(found) => return Err(found),
            crate::history::Named::Nothing => return Err(Vec::new()),
        };
        let load = |file: &Path| yi_session::load_session(file).map_err(|_| Vec::new());
        let mut store = load(&file)?;
        let mut used = top.len();
        // Invariant: another session's trail is data, so a child's path stays below its parent's.
        for name in rest.split('/').filter(|name| !name.is_empty()) {
            let base = file.parent().ok_or_else(Vec::new)?;
            let Some(child) = crate::subagent::trail::spawned_path(&store, base, name) else {
                break;
            };
            let inside = (child.strip_prefix(base)).is_ok_and(|below| {
                (below.components()).all(|part| matches!(part, std::path::Component::Normal(_)))
            });
            if !inside {
                break;
            }
            store = load(&child)?;
            file = child;
            used = used.saturating_add(1).saturating_add(name.len());
        }
        Ok((
            Transcript::Kept(Arc::new(std::sync::Mutex::new(store))),
            used,
        ))
    }

    fn unreachable(&self) -> Option<(&'static str, bool)> {
        let why =
            "under a history wall a session is read by its id, never by a name or `root/child`";
        (!self.names).then_some((why, true))
    }

    fn corpus(&self) -> Option<crate::history::RepoSessions> {
        let mut listed =
            crate::history::repo_sessions(self.sessions_dir.as_ref()?, Path::new(&self.cwd));
        listed
            .files
            .sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        Some(listed)
    }
}

impl Resolver {
    /// The transcript a `history://` path names and the entry part after it: ids first along
    /// the whole path and each prefix, names only after, and the wall judges every spelling.
    pub(super) fn locate(
        &self,
        url: &Url,
    ) -> Result<(yi_session::SharedSession, Option<String>), FetchError> {
        let path = url.path();
        let desk = self.transcripts();
        let found = match self.history_root(path) {
            Some(hit) => Ok(hit),
            None => (desk.map_or(Err(Vec::new()), |desk| desk.open_named(path))).map(
                |(transcript, used)| (transcript.session(), path.get(used.saturating_add(1)..)),
            ),
        };
        let (session, entry) = found.map_err(|candidates| {
            let why = desk.and_then(|desk| desk.unreachable());
            match candidates.is_empty() {
                true => unresolved(url, path, &[], why),
                false => {
                    let top = path.split('/').next().unwrap_or(path);
                    unresolved(url, top, &candidates, why)
                }
            }
        })?;
        self.spellings_walled(url, &session, entry)?;
        Ok((session, entry.map(str::to_owned)))
    }

    /// Invariant: a wall names a session one way and a read may name it another, so the read
    /// is judged as its id, its name as an address resolves it (lowercase) and its name as typed.
    pub(super) fn spellings_walled(
        &self,
        url: &Url,
        session: &yi_session::SharedSession,
        entry: Option<&str>,
    ) -> Result<(), FetchError> {
        let (id, name) = {
            let store = yi_session::lock_session(session);
            (store.metadata().id.clone(), store.name())
        };
        let slug = name.as_deref().map(crate::history::slug);
        let dashed = name
            .as_deref()
            .map(|name| name.trim().replace(char::is_whitespace, "-"));
        let tail = entry.map(|entry| format!("/{entry}")).unwrap_or_default();
        for spelling in std::iter::once(id).chain(slug).chain(dashed) {
            let Ok(other) = format!("history://{spelling}{tail}").parse::<Url>() else {
                continue;
            };
            if let Some(refusal) = self.wall().check_url(&other, &self.workspace) {
                return Err(FetchError::Denied {
                    url: url.to_string(),
                    refusal,
                });
            }
        }
        Ok(())
    }
}

/// Candidates a shared name lists before a `[…]` row names the rest.
const CANDIDATES_SHOWN: usize = 8;

/// No transcript opened: a name several sessions carry lists them by the address that reads
/// each, and a desk that cannot reach what another role's could says why.
fn unresolved(
    url: &Url,
    agent: &str,
    candidates: &[crate::history::Candidate],
    why: Option<(&str, bool)>,
) -> FetchError {
    match why.filter(|_| candidates.is_empty()) {
        Some((why, true)) => {
            let refusal = format!("{agent} opened no session by id, and {why}");
            return FetchError::Denied {
                url: url.to_string(),
                refusal,
            };
        }
        Some((why, false)) => {
            let what = format!("agent {agent}: {why}");
            return FetchError::NotFound {
                url: url.to_string(),
                what,
            };
        }
        None => {}
    }
    if candidates.is_empty() {
        return FetchError::NotFound {
            url: url.to_string(),
            what: format!(
                "agent {agent}: not the attached session, not a live child, and no session on disk carries that id or name"
            ),
        };
    }
    let mut rows: Vec<String> = (candidates.iter().take(CANDIDATES_SHOWN))
        .map(|candidate| format!("history://{} (in {})", candidate.id, candidate.cwd))
        .collect();
    if candidates.len() > CANDIDATES_SHOWN {
        rows.push(format!(
            "[… {CANDIDATES_SHOWN} of {} shown · at most {CANDIDATES_SHOWN} · rename the one you mean, or read by id]",
            candidates.len()
        ));
    }
    FetchError::BadAddress {
        url: url.to_string(),
        detail: format!(
            "{} sessions are named {agent:?}; read one by its address:\n{}",
            candidates.len(),
            rows.join("\n")
        ),
    }
}
