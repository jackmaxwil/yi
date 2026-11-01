//! Kept rules (#600 stage 2c): what an "always" keeps, journaled to the holder's own session,
//! replayed on `--continue` and copied down to a child, never up.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use yi_permission::SessionRules;
use yi_types::permission::{RuleDecision, RuleKind, SessionPermissionRule};

use super::{Journal, PermissionBroker};

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
            claude_hooks: self.claude_hooks.clone(),
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
        // `sandbox.egress` is the person's own config, so a child's contained command meets it too.
        if let Some(egress) = self.egress.config.get() {
            let _first_wiring_wins = child.egress.config.set(egress.clone());
        }
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
                !protected(Some(&walled), &dir) && !protected(Some(&walled), &real)
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
    /// that needs the host, a retry of a pathless refusal, or one in a protected dir.
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
        protected(Some(sandbox), &dir).then(|| crate::gate::walled_refusal(path))
    }

    /// Makes `dir` writable to every contained run, as an "always" on a widened retry would, but
    /// never journaled (it runs before the session store attaches): `--continue` names it again.
    pub fn grant_write(&self, dir: &Path) -> Result<(), String> {
        let real = dir
            .canonicalize()
            .map_err(|error| format!("{}: {error}", dir.display()))?;
        let holds = self.sandbox.as_ref().is_some_and(|sandbox| {
            sandbox
                .writable
                .iter()
                .any(|root| root == &real || root.canonicalize().is_ok_and(|root| root == real))
        });
        if holds {
            return Ok(());
        }
        if self.sandbox.is_none() {
            return Ok(());
        }
        if protected_from_grant(self.sandbox.as_ref(), &real) {
            return Err(format!(
                "{} is protected and cannot be granted",
                real.display()
            ));
        }
        let grant = yi_permission::write_grant(&real);
        self.keep_grant(
            Some(&grant),
            RuleKind::Command,
            &grant.canonical,
            "--allow-write",
        );
        Ok(())
    }

    /// The session store in use, when `--session-dir` moved it from `~/.yi/sessions`.
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
        if let Some(rule) = kept
            && let Some(journal) = self.rule_journal.get()
        {
            journal(rule);
        }
    }
}

/// Never widened, even through a link: `$HOME`, `~/.yi` (harness, MCP store, config, sessions,
/// daemon socket), a credential store, or a git path the host runs.
fn never_widened(sandbox: Option<&yi_tools::Sandbox>) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let yi = home.iter().map(|home| home.join(".yi"));
    let sandboxed = (sandbox.into_iter()).flat_map(|sandbox| {
        sandbox
            .deny_read
            .iter()
            .chain(&sandbox.deny_write)
            .chain(&sandbox.host_owned)
            .chain(sandbox.sealed.iter().map(|sealed| &sealed.denied))
            .cloned()
            .collect::<Vec<_>>()
    });
    yi.chain(sandboxed).collect()
}

fn above_home(dir: &Path) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.is_some_and(|home| {
        home.starts_with(dir) || home.canonicalize().is_ok_and(|real| real.starts_with(dir))
    })
}

fn is_home(dir: &Path) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.is_some_and(|home| dir == home || home.canonicalize().is_ok_and(|real| dir == real))
}

/// A refusal asks from both sides, because the gate judges the refused file's parent: an approval
/// for `<trunk>/.git/config` widens the run by `<trunk>/.git`.
pub(crate) fn protected(sandbox: Option<&yi_tools::Sandbox>, dir: &Path) -> bool {
    is_home(dir)
        || never_widened(sandbox).iter().any(|guarded| {
            let real = guarded.canonicalize().unwrap_or_else(|_| guarded.clone());
            // Invariant: a `.git` pointer guards itself; granting the tree around it opens no
            // store, and the deny still outranks the grant.
            let pointer = guarded.file_name() == Some(".git".as_ref()) && !guarded.is_dir();
            [guarded, &real]
                .iter()
                .any(|guarded| dir.starts_with(guarded) || (!pointer && guarded.starts_with(dir)))
        })
}

/// A grant asks only at or below: a deny outranks the grant around it. Invariant: `$HOME` and
/// `~/.yi` carry no deny, so a directory above them is refused, or `--allow-write /` opens all home.
pub(crate) fn protected_from_grant(sandbox: Option<&yi_tools::Sandbox>, dir: &Path) -> bool {
    // With no profile nothing is contained, so no grant widens anything and none is refused.
    (sandbox.is_some() && (is_home(dir) || above_home(dir)))
        || never_widened(sandbox).iter().any(|guarded| {
            let real = guarded.canonicalize().unwrap_or_else(|_| guarded.clone());
            dir.starts_with(guarded) || dir.starts_with(&real)
        })
}

#[cfg(test)]
mod tests {
    use super::{protected, protected_from_grant};
    use std::path::{Path, PathBuf};

    fn profile(tree: &str) -> yi_tools::Sandbox {
        yi_tools::Sandbox::for_workspace(Path::new(tree), Path::new("/nonexistent-home"), None)
    }

    /// Dies with a `.git` that is no directory guarding every directory above it: `--allow-write`
    /// on a parent of a non-repository cwd exited 2 naming protection that was not real.
    #[test]
    fn a_pointer_guards_itself_and_not_the_trees_above_it() {
        let sandbox = profile("/nonexistent-tree/sub");
        assert!(!protected(Some(&sandbox), Path::new("/nonexistent-tree")));
        assert!(!protected(
            Some(&sandbox),
            Path::new("/nonexistent-tree/sub")
        ));
        assert!(protected(
            Some(&sandbox),
            &PathBuf::from("/nonexistent-tree/sub/.git/x")
        ));
    }

    /// Dies with a grant judged from both sides: `--allow-write .` in a checkout exited 2 naming
    /// protection that was not real, while the deny outranks any grant there anyway.
    #[test]
    fn a_grant_of_the_tree_around_a_denial_is_taken() {
        let sandbox = yi_tools::Sandbox {
            writable: vec![PathBuf::from("/repo")],
            deny_read: Vec::new(),
            deny_write: vec![PathBuf::from("/repo/.git/config")],
            sealed: Vec::new(),
            host_owned: Vec::new(),
            spared: Vec::new(),
        };
        assert!(
            protected(Some(&sandbox), Path::new("/repo")),
            "a refusal names the tree an approval would widen"
        );
        assert!(!protected_from_grant(Some(&sandbox), Path::new("/repo")));
        assert!(protected_from_grant(
            Some(&sandbox),
            Path::new("/repo/.git/config")
        ));
    }
}
