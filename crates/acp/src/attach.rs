//! Incident: a switch held the worker's only request loop 250-550 ms on a lane claim. A build
//! now runs on its own thread; a request that needs the live session waits for it.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use serde_json::Value;
use yi_runtime::session_store::{SharedSession, lock_session};
use yi_runtime::{AgentSession, SubagentHost};

use crate::update::extension;
use crate::update::update_notification;
use crate::{AcpState, bridge_asker};

pub(crate) type Built = Result<(AgentSession, Arc<SubagentHost>), String>;

pub(crate) struct Building {
    pub(crate) store: SharedSession,
    done: Receiver<Built>,
}

pub(crate) enum Incoming {
    Line(String),
    Built(String),
}

impl AcpState {
    pub(crate) fn attach_later(&mut self, store: &SharedSession) -> String {
        let session_id = lock_session(store).metadata().id.clone();
        self.failed.remove(&session_id);
        let asker = bridge_asker(
            session_id.clone(),
            Arc::clone(&self.sink),
            Arc::clone(&self.pending),
        );
        let (build, wake) = (Arc::clone(&self.build), self.wake.upgrade());
        let (done_tx, done) = std::sync::mpsc::channel();
        let id = session_id.clone();
        let spawned = std::thread::Builder::new()
            .name("yi-attach".to_owned())
            .spawn(move || {
                yi_types::trace::name_thread("attach");
                let built = {
                    let _span = yi_types::trace::span("acp.build_session").arg("session", &*id);
                    build(Some(asker), Some(&id))
                };
                let _ = done_tx.send(built);
                if let Some(wake) = wake {
                    let _ = wake.send(Incoming::Built(id));
                }
            });
        if let Err(error) = spawned {
            self.failed
                .insert(session_id.clone(), format!("attach thread: {error}"));
            return session_id;
        }
        self.building.insert(
            session_id.clone(),
            Building {
                store: Arc::clone(store),
                done,
            },
        );
        session_id
    }

    pub(crate) fn settle(&mut self, session_id: &str) {
        let Some(building) = self.building.remove(session_id) else {
            return;
        };
        let built = building
            .done
            .recv()
            .unwrap_or_else(|_| Err("the session build thread died".to_owned()));
        match self.adopt(&building.store, built) {
            Ok(_) => self.emit_config(session_id),
            Err(error) => {
                let text = format!("the session could not start: {error}");
                let update = extension("_yi/notice", [("text", Value::String(text))]);
                (self.sink)(&update_notification(session_id, update));
                self.failed.insert(session_id.to_owned(), error);
            }
        }
    }

    pub(crate) fn store_of(&self, session_id: &str) -> Option<SharedSession> {
        match self.sessions.get(session_id) {
            Some(handle) => handle.session.store(),
            None => self
                .building
                .get(session_id)
                .map(|building| Arc::clone(&building.store)),
        }
    }
}
