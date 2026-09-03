use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::SessionError;
use crate::id::{IdGenerator, now_ms, validate_session_id};
use crate::query::{CreateOptions, ForkScope, SessionMetadata};
use crate::store::SessionStore;

pub type SharedSession = Arc<Mutex<SessionStore>>;

pub fn lock_session(session: &SharedSession) -> MutexGuard<'_, SessionStore> {
    session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub trait SessionRepo {
    fn create(&mut self, options: CreateOptions) -> Result<SharedSession, SessionError>;
    fn open(&mut self, id: &str) -> Result<SharedSession, SessionError>;
    fn list(&mut self) -> Result<Vec<SessionMetadata>, SessionError>;
    fn delete(&mut self, id: &str) -> Result<(), SessionError>;
    fn fork(
        &mut self,
        source_id: &str,
        scope: &ForkScope,
        options: CreateOptions,
    ) -> Result<SharedSession, SessionError>;
}

#[derive(Default)]
pub struct MemRepo {
    sessions: HashMap<String, SharedSession>,
    ids: IdGenerator,
}

impl MemRepo {
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve_new_id(&mut self, requested: Option<String>) -> Result<String, SessionError> {
        let id = requested.unwrap_or_else(|| self.ids.next_id());
        validate_session_id(&id)?;
        if self.sessions.contains_key(&id) {
            return Err(SessionError::AlreadyExists(format!(
                "Session already exists: {id}"
            )));
        }
        Ok(id)
    }
}

impl SessionRepo for MemRepo {
    fn create(&mut self, options: CreateOptions) -> Result<SharedSession, SessionError> {
        let id = self.resolve_new_id(options.id)?;
        let store = SessionStore::in_memory(SessionMetadata {
            id: id.clone(),
            created_at: now_ms(),
            parent_session_id: options.parent_session_id,
            name: None,
        });
        let shared = Arc::new(Mutex::new(store));
        self.sessions.insert(id, Arc::clone(&shared));
        Ok(shared)
    }

    fn open(&mut self, id: &str) -> Result<SharedSession, SessionError> {
        self.sessions
            .get(id)
            .map(Arc::clone)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {id}")))
    }

    fn list(&mut self) -> Result<Vec<SessionMetadata>, SessionError> {
        let mut listed: Vec<SessionMetadata> = self
            .sessions
            .values()
            .map(|session| lock_session(session).metadata().clone())
            .collect();
        listed.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(listed)
    }

    fn delete(&mut self, id: &str) -> Result<(), SessionError> {
        self.sessions.remove(id);
        Ok(())
    }

    fn fork(
        &mut self,
        source_id: &str,
        scope: &ForkScope,
        options: CreateOptions,
    ) -> Result<SharedSession, SessionError> {
        let source = self.open(source_id)?;
        let mutations = lock_session(&source).fork_mutations(scope)?;
        let parent = options
            .parent_session_id
            .clone()
            .or_else(|| Some(source_id.to_owned()));
        let id = self.resolve_new_id(options.id)?;
        let mut store = SessionStore::in_memory(SessionMetadata {
            id: id.clone(),
            created_at: now_ms(),
            parent_session_id: parent,
            name: None,
        });
        for mutation in mutations {
            store.replay(mutation)?;
        }
        let shared = Arc::new(Mutex::new(store));
        self.sessions.insert(id, Arc::clone(&shared));
        Ok(shared)
    }
}
