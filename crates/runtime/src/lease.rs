//! A child's lease: drawn from its parent at spawn, revoked with a grace, and repossessed with
//! a record that lands before the child's own record is released (plan section 7.4, D215).
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use yi_types::lease::{
    Disposition, Lease, LeaseRecord, ParentClose, Repossession, Returned, Revocation,
};
use yi_types::mail::Kind;
use yi_types::message::AgentMessage;
use yi_types::plan::op::Choice;
use yi_types::subagent::ChildExit;

use crate::args::Args;
use crate::mail::Draft;
use crate::subagent::{ChildRecord, Children, PARENT_NAME, Step, SubagentHost};

pub(crate) const LEASE_ENTRY: &str = "lease";
/// What a parent keeps of its own clock to settle and record the children it outlives.
pub const CLEANUP_GRACE_MS: u64 = 30_000;
/// How long a stopped run may take to go idle before its repossession is left pending.
const JOIN_MS: u64 = 10_000;

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
pub type Timer = Arc<dyn Fn(Duration) + Send + Sync>;

/// What this host itself holds, which is all its children can draw on.
#[derive(Default)]
pub(crate) struct Grant {
    pub(crate) wall: crate::wall::Wall,
    tokens: Option<u64>,
    spent: u64,
    clock: Option<Clock>,
    timer: Option<Timer>,
    /// The journal is read for revocations a restart interrupted once, on the first expiry.
    resumed: bool,
}

/// What a spawn asks of its parent's lease.
pub(crate) struct Ask {
    deadline_ms: Option<u64>,
    pub(crate) tokens: Option<u64>,
    pub(crate) parent_close: ParentClose,
}

impl Ask {
    pub(crate) fn from_kwargs(kwargs: &Map<String, Value>) -> Result<Self, String> {
        let number = |key: &str| match kwargs.get(key).filter(|value| !value.is_null()) {
            None => Ok(None),
            Some(value) => value
                .as_u64()
                .filter(|asked| *asked > 0)
                .map(Some)
                .ok_or_else(|| format!("rlm.run {key} must be a positive whole number")),
        };
        let parent_close = match kwargs.get("parent_close").filter(|value| !value.is_null()) {
            None => ParentClose::default(),
            Some(asked) => {
                let named = |name: &str| json!({"policy": name});
                let policy = asked.as_str().map_or_else(|| asked.clone(), named);
                if policy["policy"] == "abandon" {
                    return Err("parent_close \"abandon\" is refused: nothing owns an orphan's address, budget, inbox, artifacts and deadline until a supervisor exists. Use \"terminate\" or \"request_cancel\"".to_owned());
                }
                serde_json::from_value(policy)
                    .map_err(|error| format!("parent_close {asked}: {error}"))?
            }
        };
        Ok(Self {
            deadline_ms: number("deadline_s")?.map(|seconds| seconds.saturating_mul(1_000)),
            tokens: number("tokens")?,
            parent_close,
        })
    }
}

impl SubagentHost {
    /// Walls only shrink: a child's is its own under everything this host is walled by.
    pub(crate) fn wall_for(
        &self,
        kwargs: &Map<String, Value>,
    ) -> Result<crate::wall::Wall, String> {
        let grant = self.grant.lock().map_err(|_| "lease state poisoned")?;
        Ok(crate::wall::Wall::from_kwargs(kwargs, &self.options.cwd)?.under(&grant.wall))
    }

    /// The wall and tokens this host was itself granted; a root holds neither.
    pub fn set_grant(&self, wall: crate::wall::Wall, tokens: Option<u64>) {
        if let Ok(mut grant) = self.grant.lock() {
            grant.wall = wall;
            grant.tokens = tokens;
        }
    }

    /// The lease clock in epoch milliseconds, and where a grace's due time is registered:
    /// the probe loop's `wake_at`, so an earlier due time interrupts its sleep.
    pub fn set_lease_clock(&self, clock: Option<Clock>, timer: Option<Timer>) {
        if let Ok(mut grant) = self.grant.lock() {
            grant.clock = clock.or(grant.clock.take());
            grant.timer = timer.or(grant.timer.take());
        }
    }

    pub(crate) fn lease_now(&self) -> u64 {
        let clock = self.grant.lock().ok().and_then(|grant| grant.clock.clone());
        clock.map_or_else(yi_session::now_ms, |clock| clock())
    }

    /// Invariant: a lease is drawn, never minted: an ask past what this host holds is refused
    /// with both numbers, never clamped. Runs under the roster lock, so no token is drawn twice.
    pub(crate) fn draw(&self, children: &Children, name: &str, ask: &Ask) -> Result<Lease, String> {
        let left = self.deadline().map(|ends| {
            let remaining = ends.saturating_duration_since(Instant::now()).as_millis();
            u64::try_from(remaining).unwrap_or(u64::MAX)
        });
        if left == Some(0) {
            return Err(
                "the parent's own deadline has passed, so it has no time to lease".to_owned(),
            );
        }
        let bound = left.map(|left| left.saturating_sub(CLEANUP_GRACE_MS));
        if let (Some(asked), Some(bound)) = (ask.deadline_ms, bound)
            && asked > bound
        {
            return Err(format!(
                "deadline_s asks for {asked} ms and the parent has {bound} ms to lease ({CLEANUP_GRACE_MS} ms of its clock is kept for cleanup); nothing is clamped, ask for less"
            ));
        }
        let now = self.lease_now();
        let grant = self.grant.lock().map_err(|_| "lease state poisoned")?;
        if let (Some(asked), Some(held)) = (ask.tokens, grant.tokens) {
            let available = held
                .saturating_sub(grant.spent)
                .saturating_sub(children.reserved());
            if asked > available {
                return Err(format!(
                    "tokens asks for {asked} and the parent has {available} of its {held} unreserved; nothing is clamped, ask for less or reap a child"
                ));
            }
        }
        Ok(Lease {
            holder: name.to_owned(),
            parent: PARENT_NAME.to_owned(),
            deadline_ms: ask
                .deadline_ms
                .or(bound)
                .map(|span| now.saturating_add(span)),
            tokens: ask.tokens,
            granted_at: now,
            revoked: None,
        })
    }

    fn journal(&self, record: &LeaseRecord) -> Result<(), String> {
        let store =
            (self.options.store)().ok_or("the parent has no transcript to journal a lease on")?;
        yi_session::lock_session(&store)
            .append_custom_record(record)
            .map(drop)
            .map_err(|error| format!("the lease journal refused the record: {error}"))
    }

    /// Invariant: the reservation comes back exactly once, as the record leaves. A turn that
    /// reported no usage is an unknown spend: the whole reservation stays spent.
    pub(crate) fn return_lease(&self, record: &ChildRecord) {
        self.journal_settled(self.settle_lease(record));
    }

    /// The accounting half of [`Self::return_lease`], and all of it a caller may run under the
    /// roster lock: the line it hands back is journaled once that lock is down.
    pub(crate) fn settle_lease(&self, record: &ChildRecord) -> Option<LeaseRecord> {
        if record.lease.tokens.is_none() && record.lease.revoked.is_none() {
            return None;
        }
        let reserved = record.lease.tokens.unwrap_or(0);
        let spent = record.token_count();
        let unknown = record.billable(&record.session.messages()).iter().any(
            |message| matches!(message, AgentMessage::Assistant { usage, .. } if usage.total_tokens == 0),
        );
        let unspent = (!unknown && reserved > 0).then(|| reserved.saturating_sub(spent));
        if let Ok(mut grant) = self.grant.lock() {
            grant.spent = grant
                .spent
                .saturating_add(reserved.saturating_sub(unspent.unwrap_or(0)));
        }
        // A repossession's own record already carries the lease; a second line would reopen it.
        if record.exit == Some(ChildExit::Repossessed) {
            return None;
        }
        Some(LeaseRecord::Returned(Returned {
            lease: record.lease.clone(),
            spent,
            unspent,
        }))
    }

    pub(crate) fn journal_settled(&self, settled: Option<LeaseRecord>) {
        if let Some(record) = settled {
            let _a_failed_write_never_blocks_a_reap = self.journal(&record);
        }
    }

    /// `rlm.revoke`: the revocation is journaled, then a `cancel` reaches the child, then the
    /// grace's due time is registered. The child that stops inside the grace keeps its record.
    pub fn revoke(
        &self,
        target: &str,
        grace_ms: u64,
        reason: &str,
    ) -> Result<Map<String, Value>, String> {
        let mut lease = {
            let children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let key = Self::key_of(&children, target)?;
            let record = children.get(&key).ok_or("the child is gone")?;
            if record.exit.is_some() {
                return Err(format!(
                    "child \"{target}\" has already ended; reap it instead"
                ));
            }
            record.lease.clone()
        };
        lease.revoked = Some(Revocation {
            at: self.lease_now(),
            grace_ms,
            reason: reason.to_owned(),
        });
        self.journal(&LeaseRecord::Revoked(lease.clone()))?;
        let name = lease.holder.clone();
        if let Ok(mut children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, &name)
            && let Some(record) = children.get_mut(&key)
        {
            record.lease = lease;
            record.standing.stop();
            children.touch(&key, crate::family::Cause::Revoked);
        }
        let cancel = Draft::of(Kind::Cancel, reason);
        let mut reply = self.route_mail(PARENT_NAME, &name, &cancel)?;
        let timer = self.grant.lock().ok().and_then(|grant| grant.timer.clone());
        if let Some(timer) = timer {
            timer(Duration::from_millis(grace_ms));
        }
        reply.insert("revoked".to_owned(), Value::String(name));
        reply.insert("grace_ms".to_owned(), Value::from(grace_ms));
        Ok(reply)
    }

    /// The two ways a parent stops a child: at once, or with a grace and its work kept.
    pub(crate) fn register_stops(self: &Arc<Self>, registry: &mut crate::kernel::HostRegistry) {
        let host = Arc::clone(self);
        registry.register("rlm.interrupt", move |payload| {
            let target = payload.str_of("target").map(str::to_owned);
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.interrupt requires a target")?;
                host.interrupt(&target)
            })
        });
        let host = Arc::clone(self);
        registry.register("rlm.revoke", move |payload| {
            let text = |key: &str| payload.str_of(key).map(str::to_owned);
            let (target, reason) = (text("target"), text("reason").unwrap_or_default());
            let grace = payload.u64_of("grace_ms");
            let host = Arc::clone(&host);
            Box::pin(async move {
                let target = target.ok_or("rlm.revoke requires a target")?;
                host.revoke(
                    &target,
                    grace.unwrap_or(yi_types::lease::DEFAULT_GRACE_MS),
                    &reason,
                )
            })
        });
    }

    /// The parent is closing: every live child is revoked under its own `parent_close`.
    pub fn close(&self) -> Vec<String> {
        let live: Vec<(String, u64)> = self
            .children
            .lock()
            .map(|mut children| {
                // An idle service has no run to revoke; the close still ends it for good.
                children
                    .values_mut()
                    .for_each(|record| record.standing.stop());
                children
                    .values()
                    .filter(|record| record.exit.is_none() && record.lease.revoked.is_none())
                    .map(|record| (record.session_name.clone(), record.parent_close.grace_ms()))
                    .collect()
            })
            .unwrap_or_default();
        live.into_iter()
            .filter_map(|(name, grace)| {
                self.revoke(&name, grace, "the parent closed")
                    .ok()
                    .map(|_| name)
            })
            .collect()
    }

    /// The timer's job, run on every wake of the probe loop: each revoked child whose grace is
    /// over and whose run has not ended is repossessed. A pending one is tried again.
    pub async fn expire(self: &Arc<Self>) -> Vec<String> {
        let first = self
            .grant
            .lock()
            .is_ok_and(|mut grant| !std::mem::replace(&mut grant.resumed, true));
        if first && let Err(reason) = self.resume_revocations() {
            (self.options.notice)(&format!("[lease resume refused: {reason}]"), None);
        }
        let now = self.lease_now();
        let due: Vec<String> = self
            .children
            .lock()
            .map(|children| {
                children
                    .iter()
                    .filter(|(_, record)| {
                        let revoked = record.lease.revoked.as_ref();
                        record.exit.is_none() && revoked.is_some_and(|revoked| revoked.due() <= now)
                    })
                    .map(|(key, _)| key.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut repossessed = Vec::new();
        for key in due {
            match self.repossess(&key).await {
                Ok(Some(name)) => repossessed.push(name),
                Ok(None) => {}
                Err(reason) => self.leave_pending(&key, &reason),
            }
        }
        repossessed
    }

    /// Invariant: a stop, a settle or a record that failed leaves the child visible as
    /// `repossession_pending` with everything it held still on its record, never a clean end.
    fn leave_pending(&self, key: &str, reason: &str) {
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(key)
        {
            record.step(Step::Pending(format!("repossession pending: {reason}")));
            children.touch(key, crate::family::Cause::Revoked);
        }
    }

    /// Invariant: nothing below the join runs while the run can still write, and the record
    /// with the kept references is journaled before the child's own record is released.
    async fn repossess(self: &Arc<Self>, key: &str) -> Result<Option<String>, String> {
        let session = {
            let mut children = self
                .children
                .lock()
                .map_err(|_| "subagent state poisoned")?;
            let record = children.get_mut(key).ok_or("the child is gone")?;
            if record.exit.is_some() {
                return Ok(None);
            }
            record.step(Step::Repossess);
            record.disposition.get_or_insert(Choice::Retained);
            Arc::clone(&record.session)
        };
        session.abort();
        let joined = tokio::time::timeout(Duration::from_millis(JOIN_MS), session.wait_idle());
        if joined.await.is_err() {
            return Err(format!("its run did not stop within {JOIN_MS} ms"));
        }
        let at = self.lease_now();
        let (_, record, _) =
            self.retire_as(key, ChildExit::Repossessed, &|record, candidate| {
                let kept = [
                    Some(format!("history://{}", record.session_name)),
                    candidate.map(|candidate| format!("branch://{}", candidate.branch.as_str())),
                ];
                self.journal(&LeaseRecord::Repossessed(Repossession {
                    lease: record.lease.clone(),
                    at,
                    kept: kept
                        .into_iter()
                        .flatten()
                        .filter_map(|url| url.parse().ok())
                        .collect(),
                    disposition: Disposition::Settled,
                }))
            })?;
        if let Some(store) = record.session.store()
            && let Ok(mut reaped) = self.reaped.lock()
        {
            reaped.insert(record.session_name.clone(), store);
        }
        let reason = record
            .lease
            .revoked
            .map(|revoked| revoked.reason)
            .unwrap_or_default();
        let text = format!(
            "repossessed after its grace: {reason}. Its transcript is history://{}",
            record.session_name
        );
        let failure = Draft::of(Kind::Failure, &text);
        let _the_record_is_already_journaled =
            self.route_mail(&record.session_name, "parent", &failure);
        Ok(Some(record.session_name))
    }

    /// After a restart during a grace: every journaled revocation with no repossession after
    /// it is completed from the journal alone, since the child it named died with the host.
    pub fn resume_revocations(&self) -> Result<Vec<String>, String> {
        let Some(store) = (self.options.store)() else {
            return Ok(Vec::new());
        };
        let entries = yi_session::lock_session(&store)
            .find_entries(&yi_session::EntryQuery {
                custom_type: Some(LEASE_ENTRY.to_owned()),
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            })
            .map_err(|error| error.to_string())?;
        let mut open: Vec<Lease> = Vec::new();
        for entry in entries {
            let yi_types::entry::Entry::Custom {
                data: Some(data), ..
            } = entry
            else {
                continue;
            };
            match serde_json::from_value::<LeaseRecord>(data) {
                Ok(LeaseRecord::Revoked(lease)) => open.push(lease),
                Ok(LeaseRecord::Repossessed(done)) => {
                    open.retain(|lease| lease.holder != done.lease.holder)
                }
                Ok(LeaseRecord::Returned(done)) => {
                    open.retain(|lease| lease.holder != done.lease.holder)
                }
                Err(error) => {
                    return Err(format!(
                        "a lease record this build cannot read ({error}) may have closed an open lease, so none was completed past it"
                    ));
                }
            }
        }
        open.retain(|lease| !self.holds(&lease.holder));
        for lease in &open {
            let kept = format!("history://{}", lease.holder).parse().ok();
            self.journal(&LeaseRecord::Repossessed(Repossession {
                lease: lease.clone(),
                at: self.lease_now(),
                kept: kept.into_iter().collect(),
                // Nothing settled here: the host that held the lane died with the child.
                disposition: Disposition::Pending {
                    reason:
                        "the host restarted inside the grace; a worktree it held is an orphan lane"
                            .to_owned(),
                },
            }))?;
            (self.options.notice)(
                &format!(
                    "[child {} repossessed: its revocation was completed after a restart; any worktree it held is an orphan lane]",
                    lease.holder
                ),
                None,
            );
        }
        Ok(open.into_iter().map(|lease| lease.holder).collect())
    }
}
