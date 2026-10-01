//! Kept rules (#600 stage 2c): what an "always" keeps, journaled to the holder's own session,
//! replayed on `--continue` and copied down to a child, never up.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use yi_permission::SessionRules;
use yi_types::permission::{RuleDecision, RuleKind, SessionPermissionRule};

use super::{Journal, PermissionBroker, protected};

impl PermissionBroker {
    /// A child's broker: the family's mode, holds, asker and reviewers, and a copy of the kept
    /// rules its wall and `cwd` admit. What it keeps stays its own, so nothing flows back up.
    #[must_use]
    pub fn for_child(&self, wall: &crate::wall::Wall, cwd: &Path) -> Self {
        let mut rules = self.lock_rules().state().clone();
        // A pass names the directory it was kept in; a child elsewhere never ran it there.
        let elsewhere = cwd != self.cwd;
        rules.rules.retain(|rule| {
            self.admits(rule, wall) && !(elsewhere && yi_permission::is_exact_command(rule))
        });
        let mut child = Self {
            sandbox: self.sandbox.clone(),
            mode: Arc::clone(&self.mode),
            config_rules: self.config_rules.clone(),
            session_rules: Mutex::new(SessionRules::load(rules).unwrap_or_default()),
            holds: Arc::clone(&self.holds),
            reviewer: self.reviewer.clone(),
            journal: self.journal.clone(),
            approver: self.approver.clone(),
            prompts_close_on_settle: std::sync::atomic::AtomicBool::new(
                self.prompts_close_on_settle.load(Ordering::Relaxed),
            ),
            walled: self.walled || !wall.is_empty(),
            ..Self::new(
                self.mode(),
                self.cwd.clone(),
                Vec::new(),
                self.asker.clone(),
                self.events.clone(),
            )
        };
        // The `--session-dir` in use stays host-owned below the root too (D335, #971).
        child
            .context
            .host_owned
            .clone_from(&self.context.host_owned);
        child
    }

    /// A write grant holds while its directory is neither protected nor walled; a pass, which
    /// runs outside every wall, never in a walled holder. Path rules meet D323 at each call.
    fn admits(&self, rule: &SessionPermissionRule, wall: &crate::wall::Wall) -> bool {
        if let Some(dir) = yi_permission::write_grant_dir(rule) {
            return self.sandbox.as_ref().is_some_and(|sandbox| {
                let mut walled = sandbox.clone();
                walled.deny_write.extend_from_slice(&wall.deny_write);
                walled.deny_read.extend_from_slice(&wall.deny_read);
                // A directory swapped for a link since it was kept is judged where it leads.
                let real = dir.canonicalize().unwrap_or_else(|_| dir.clone());
                !protected(&walled, &dir) && !protected(&walled, &real)
            });
        }
        wall.is_empty() || !yi_permission::is_exact_command(rule)
    }

    /// `--continue`: the rules the session's ledger kept, re-checked against today's wall and
    /// credential list; a rule whose path is protected now is dropped, never widened.
    pub fn replay(&self, kept: Vec<SessionPermissionRule>, wall: &crate::wall::Wall) {
        let admitted: Vec<_> = kept
            .into_iter()
            .filter(|rule| self.admits(rule, wall))
            .collect();
        let mut rules = self.lock_rules();
        for rule in admitted {
            let _cap_is_soft = rules.insert(
                rule.kind,
                &rule.canonical,
                &rule.display_identity,
                rule.decision,
            );
        }
    }

    /// Each kept rule as `/permissions` lists it.
    pub fn kept_rules(&self) -> Vec<String> {
        (self.lock_rules().state().rules.iter())
            .map(|rule| rule.display_identity.clone())
            .collect()
    }

    /// Directories an "always" on a widened retry made writable to every later contained run.
    pub fn kept_writes(&self) -> Vec<PathBuf> {
        (self.lock_rules().state().rules.iter())
            .filter(|rule| rule.decision == RuleDecision::Allow)
            .filter_map(yi_permission::write_grant_dir)
            .collect()
    }

    pub(super) fn lock_rules(&self) -> std::sync::MutexGuard<'_, SessionRules> {
        self.session_rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The session store in use, when `--session-dir` moved it from `~/.yi/sessions`: no
    /// contained write and no write tool reaches the ledger kept rules replay from.
    #[must_use]
    pub fn with_session_store(mut self, dir: &Path) -> Self {
        self.context.host_owned.push(dir.to_path_buf());
        if let Some(sandbox) = &mut self.sandbox {
            sandbox.host_owned.push(dir.to_path_buf());
        }
        self
    }

    /// Invariant: where Seatbelt exists, a walled holder's call never runs unconfined, unless the
    /// user chose yolo: outside, its text would be its only wall, and text is bypassable (#1001).
    pub(crate) fn confines_walled(&self) -> bool {
        self.walled && self.sandbox.is_some() && self.mode() != crate::PermissionMode::Yolo
    }

    /// A walled holder's call that would leave the sandbox is refused before anyone is asked: one
    /// that needs the host, a retry of a pathless refusal, or one in a protected dir (D345).
    pub(super) fn walled_retry(
        &self,
        command: Option<&str>,
        refusal: Option<&yi_tools::SandboxRefusal>,
    ) -> Option<String> {
        let confined = command.filter(|_| self.confines_walled());
        if let Some((why, _)) = confined.and_then(|text| super::leaves_sandbox(text, &self.context))
        {
            return Some(crate::gate::walled_host_refusal(why));
        }
        if let (Some(_), Some(yi_tools::SandboxRefusal::Scopes(_))) = (confined, refusal) {
            return Some(crate::gate::walled_host_refusal(
                "its last contained run was refused",
            ));
        }
        let (true, Some(sandbox), Some(yi_tools::SandboxRefusal::Path(path))) =
            (self.walled, &self.sandbox, refusal)
        else {
            return None;
        };
        let dir = path.parent().unwrap_or(path);
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        protected(sandbox, &dir).then(|| crate::gate::walled_refusal(path))
    }

    /// The session stores: `~/.yi/sessions` and the `--session-dir` in use.
    pub fn session_stores(&self) -> &[PathBuf] {
        &self.context.host_owned
    }

    /// A session switch (`/new`, rpc `switch_session` or `fork`): the next session replays its own.
    pub fn forget_rules(&self) {
        *self.lock_rules() = SessionRules::new();
        if let Ok(mut failures) = self.contained_failures.lock() {
            failures.clear();
        }
    }

    pub fn set_rule_journal(&self, journal: Journal<SessionPermissionRule>) {
        let _first_wiring_wins = self.rule_journal.set(journal);
    }

    pub(super) fn keep_grant(
        &self,
        grant: Option<&yi_permission::Grant>,
        kind: RuleKind,
        canonical: &str,
        display: &str,
    ) {
        // An exact call's label names no call, so the listing adds the call it keeps.
        let (kind, canonical, label) = match grant {
            Some(grant) if grant.canonical != canonical => {
                (grant.kind, grant.canonical.as_str(), grant.label.clone())
            }
            Some(grant) => (grant.kind, canonical, format!("{}: {display}", grant.label)),
            None => (kind, canonical, display.to_owned()),
        };
        let kept = {
            let mut rules = self.lock_rules();
            let _cap_is_soft = rules.insert(kind, canonical, &label, RuleDecision::Allow);
            (rules.state().rules.iter())
                .find(|rule| rule.kind == kind && rule.canonical == canonical)
                .cloned()
        };
        if let (Some(rule), Some(journal)) = (kept, self.rule_journal.get()) {
            journal(rule);
        }
    }
}
