//! A service is a child whose name is an address: a run that crashes is respawned on the same
//! record, under the same name and transcript, within a restart intensity (plan F3c).
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::model::Model;
use yi_types::subagent::{ChildExit, FailClass};

use super::{Step, SubagentHost};

/// OTP's intensity: restarts are counted inside this window, and past `max` it stays failed.
const RESTART_WINDOW_MS: u64 = 600_000;
const DEFAULT_RESTARTS: u64 = 3;
/// The ceiling on the intensity a caller may ask for: past this a crash loop is the bound.
const MAX_RESTARTS: usize = 10;

/// Why a record stands outside the worker cap and the owner's lifecycle notice, if it does.
pub(crate) enum Standing {
    Worker,
    /// Seated by the judge tier under the verification reserve; its ending is the jury's.
    Juror,
    /// Never reaped while it serves, and idle between turns without having ended.
    Service(Service),
}

pub(crate) struct Service {
    incarnation: u32,
    max: usize,
    restarts: Vec<u64>,
    /// A revoke or the parent's close is a deliberate stop: nothing respawns after it.
    stopped: bool,
    /// A `turn_ended` reader is out on this service's current run; a second would double it.
    reading: bool,
    prompt: String,
    kwargs: Map<String, Value>,
}

impl Standing {
    pub(crate) fn incarnation(&self) -> Option<u32> {
        match self {
            Self::Service(service) => Some(service.incarnation),
            Self::Worker | Self::Juror => None,
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Self::Service(service) = self {
            service.stopped = true;
        }
    }
}

/// What `rlm.run` and `rlm.service` answer at admission.
pub(super) fn handle(
    key: &str,
    name: &str,
    session_dir: &Path,
    model: &Model,
) -> Map<String, Value> {
    let reply = serde_json::json!({
        "rlm_child_id": key,
        // A service is admitted through the same spawn, so both requests localize at `rlm.run`.
        "next": crate::affordance::next("rlm.run", &[yi_types::graph::CHILD_RUNNING], name),
        "name": name,
        "session_dir": session_dir.to_string_lossy(),
        "model": format!("{}/{}", model.provider, model.id),
    });
    reply.as_object().cloned().unwrap_or_default()
}

impl SubagentHost {
    pub(crate) fn incarnation_of(&self, name: &str) -> Option<u32> {
        let children = self.children.lock().ok()?;
        let key = Self::key_of(&children, name).ok()?;
        children.get(&key)?.standing.incarnation()
    }

    /// Invariant: a name holds one service. The same brief again attaches to it; another brief,
    /// or a name an ordinary child holds, is refused, and nothing is spawned twice.
    pub fn service(
        self: &Arc<Self>,
        name: &str,
        prompt: String,
        mut kwargs: Map<String, Value>,
        restart: usize,
    ) -> Result<Map<String, Value>, String> {
        if name.trim().is_empty() || name == super::PARENT_NAME || name == "all" {
            return Err(
                "rlm.service needs a name of its own: it is the service's address".to_owned(),
            );
        }
        if restart > MAX_RESTARTS {
            let window = RESTART_WINDOW_MS / 1_000;
            return Err(format!(
                "restart asks for {restart} respawns and this host allows {MAX_RESTARTS} within {window} s; nothing is clamped, ask for less"
            ));
        }
        if super::parse_isolation(&kwargs)? == super::Isolation::Worktree {
            return Err("a service runs in the parent's tree: a respawn has no lane to settle the last incarnation's worktree into".to_owned());
        }
        // An ordinary child holding the name falls through to the spawn's own refusal.
        if let Ok(children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, name)
            && let Some(record) = children.get(&key)
            && let Standing::Service(held) = &record.standing
        {
            if held.prompt != prompt {
                return Err(format!(
                    "service \"{name}\" already runs another brief; rlm.delete_subagent it before starting this one"
                ));
            }
            let mut reply = handle(&key, name, &record.session_dir, &record.session.model());
            reply.insert("attached".to_owned(), Value::Bool(true));
            return Ok(reply);
        }
        kwargs.insert("name".to_owned(), Value::String(name.to_owned()));
        let service = Service {
            incarnation: 1,
            max: restart,
            restarts: Vec::new(),
            stopped: false,
            reading: false,
            prompt: prompt.clone(),
            kwargs: kwargs.clone(),
        };
        self.admit(prompt, kwargs, Standing::Service(service))
    }

    /// Invariant: only a crash respawns, never a stop anyone asked for, and the new lease is
    /// drawn, never minted. `None` means the record was respawned and this ending is not one.
    pub(super) fn respawn(
        self: &Arc<Self>,
        key: &str,
        exit: ChildExit,
        error: Option<String>,
    ) -> Option<(ChildExit, Option<String>)> {
        let crashed = matches!(
            exit,
            ChildExit::Failed {
                class: FailClass::Provider | FailClass::KernelDeath
            }
        );
        let cause = error.clone().unwrap_or_default();
        let refused = |why: String| Some((exit, Some(format!("{cause}; not respawned: {why}"))));
        let (name, prompt, kwargs, dir, store, lease, next, settled) = {
            let Ok(mut children) = self.children.lock() else {
                return Some((exit, error));
            };
            let now = self.lease_now();
            let record = children.get_mut(key)?;
            let Standing::Service(service) = &mut record.standing else {
                return Some((exit, error));
            };
            if !crashed || service.stopped {
                return Some((exit, error));
            }
            service
                .restarts
                .retain(|at| now.saturating_sub(*at) < RESTART_WINDOW_MS);
            if service.restarts.len() >= service.max {
                let window = RESTART_WINDOW_MS / 1_000;
                return refused(format!(
                    "{} restarts within {window} s are spent",
                    service.max
                ));
            }
            service.restarts.push(now);
            let (prompt, kwargs) = (service.prompt.clone(), service.kwargs.clone());
            let next = service.incarnation.saturating_add(1);
            let (name, dir) = (record.session_name.clone(), record.session_dir.clone());
            let store = record.session.store();
            // The dead incarnation's lease is settled before the next is drawn against it;
            // its journal line is written once the roster lock is back down.
            let settled = self.settle_lease(record);
            record.lease.tokens = None;
            // Invariant: the kept transcript was charged to the lease that ended with it, so
            // the next incarnation is billed from where its own turns begin.
            record.billed_from = record.session.messages().len();
            let ask = crate::lease::Ask::from_kwargs(&kwargs);
            let lease = match ask.and_then(|ask| self.draw(&children, &name, &ask)) {
                Ok(lease) => lease,
                Err(why) => return refused(why),
            };
            if let Some(record) = children.get_mut(key) {
                record.lease = lease.clone();
            }
            (name, prompt, kwargs, dir, store, lease, next, settled)
        };
        self.journal_settled(settled);
        let built = self
            .cast(&kwargs)
            .and_then(|cast| self.build(cast, &name, &dir, None, &lease, store));
        let session = match built {
            Ok(session) => Arc::new(session),
            Err(why) => return refused(why),
        };
        let requested = session.abort_epoch();
        let dead = {
            let Ok(mut children) = self.children.lock() else {
                Self::dispose_child_kernel(&session);
                return None;
            };
            // Invariant: a stop the owner asked for while the build ran wins. The record is
            // read again here because nothing held it between the draw and now.
            let stopped = match children.get(key).map(|record| &record.standing) {
                Some(Standing::Service(service)) => service.stopped,
                Some(_) => true,
                None => {
                    Self::dispose_child_kernel(&session);
                    return None;
                }
            };
            if stopped {
                Self::dispose_child_kernel(&session);
                return refused("it was stopped while its next run was being built".to_owned());
            }
            let record = children.get_mut(key)?;
            let dead = std::mem::replace(&mut record.session, Arc::clone(&session));
            if let Standing::Service(service) = &mut record.standing {
                service.incarnation = next;
            }
            record.step(Step::Respawn);
            children.touch(key);
            dead
        };
        // The crashed run's kernel has no one left to read it, and nothing else disposes it.
        Self::dispose_child_kernel(&dead);
        // A request parked on the dead incarnation is refused by name, never answered by this one.
        if let Ok(mut desk) = self.mail.lock() {
            desk.drop_respondent(&name, "respawned");
        }
        let brief = format!(
            "[incarnation {next} of service \"{name}\": the run before this one ended on \"{cause}\". Your transcript and inbox were kept; a message in it addressed to an earlier incarnation was your predecessor's conversation, not yours]\n\n{prompt}"
        );
        let context = crate::mailbox::context_block(&kwargs).ok().flatten();
        self.watch(key.to_owned(), &session);
        self.publish(key);
        let (host, key) = (Arc::clone(self), key.to_owned());
        tokio::spawn(async move {
            host.run_child(key, name, brief, context, session, requested)
                .await;
        });
        None
    }

    /// A woken turn has no `run_child` behind it, so a service's later crashes are read here,
    /// one reader at a time: two readers of one crash would end it twice, on two leases.
    pub(super) fn turn_ended(self: &Arc<Self>, key: &str) {
        let held = self.children.lock().ok().and_then(|mut children| {
            let record = children.get_mut(key)?;
            let idle = record.exit == Some(ChildExit::Completed);
            let (name, session) = (record.session_name.clone(), Arc::clone(&record.session));
            let Standing::Service(service) = &mut record.standing else {
                return None;
            };
            (idle && !service.reading).then(|| {
                service.reading = true;
                (name, session)
            })
        });
        let Some((name, session)) = held else {
            return;
        };
        let (host, key) = (Arc::clone(self), key.to_owned());
        tokio::spawn(async move {
            session.wait_idle().await;
            let (exit, error) = super::exit_of(&session);
            if let Ok(mut children) = host.children.lock()
                && let Some(record) = children.get_mut(&key)
                && let Standing::Service(service) = &mut record.standing
            {
                service.reading = false;
            }
            if exit != ChildExit::Completed {
                host.conclude(&key, &name, &session, exit, error);
            }
        });
    }

    pub(super) fn register_service(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let host = Arc::clone(self);
        registry.register("rlm.service", move |payload| {
            let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
            let (name, prompt) = (text("name"), text("prompt"));
            let kwargs = payload.get("kwargs").and_then(Value::as_object).cloned();
            let restart = payload.get("restart").and_then(Value::as_u64);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let (name, prompt) = name
                    .zip(prompt)
                    .ok_or("rlm.service requires a name and a brief")?;
                let restart =
                    usize::try_from(restart.unwrap_or(DEFAULT_RESTARTS)).unwrap_or(usize::MAX);
                host.service(&name, prompt, kwargs.unwrap_or_default(), restart)
            })
        });
    }
}
