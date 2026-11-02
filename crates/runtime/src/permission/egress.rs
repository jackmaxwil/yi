//! What decides a domain for a contained spawn's network: the seed, the person's config, a kept
//! grant, then a person (#601).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use yi_permission::PermissionMode;
use yi_types::egress::Domain;
use yi_types::permission::{Answerer, RuleDecision, RuleKind};

use super::{AskOutcome, PermissionAsk, PermissionBroker};

/// The registries and hosts an install needs before anyone is asked.
const SEED: [&str; 13] = [
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "raw.githubusercontent.com",
];

/// `sandbox.egress`, and the domains whose first decision the ledger already holds.
#[derive(Default)]
pub(super) struct Named {
    pub(super) config: OnceLock<Vec<Domain>>,
    recorded: Mutex<HashSet<Domain>>,
}

impl crate::wiring::RuntimeWiring {
    /// Where a kernel `bash()` job runs, and what it asks for its proxy credentials.
    pub(crate) fn exec_home(&self) -> (std::path::PathBuf, Option<yi_tools::SpawnFn>) {
        let spawn = (self.broker.as_ref())
            .map(|broker| broker.spawn_hook(self.wall.clone(), self.cwd.clone()));
        (self.cwd.clone(), spawn)
    }
}

impl PermissionBroker {
    /// `sandbox.egress`, from the person's own config.
    #[must_use]
    pub fn with_egress(self, egress: Vec<Domain>) -> Self {
        let _first_wiring_wins = self.egress.config.set(egress);
        self
    }

    /// A contained spawn's proxy credentials and git lane, or the reason it may not start: with no
    /// proxy a contained process would have no network and no row saying why.
    pub(crate) fn spawn_hook(
        self: &Arc<Self>,
        wall: crate::wall::Wall,
        cwd: std::path::PathBuf,
    ) -> yi_tools::SpawnFn {
        let broker = Arc::clone(self);
        Arc::new(move |command: &str| {
            let lease = broker.egress_lease(wall.clone()).map_err(|error| {
                yi_tools::SpawnRefused::Egress {
                    why: error.to_string(),
                }
            })?;
            let lane = crate::lane::remote::GitLane::for_tree(&cwd);
            let refreshed = lane
                .iter()
                .flat_map(|lane| lane.refresh(command))
                .collect::<Vec<_>>();
            let assign = crate::lane::remote::GitLane::assign_for(lane.as_deref(), &cwd);
            let broker = Arc::clone(&broker);
            Ok(yi_tools::Spawn {
                env: lease.env(),
                assign,
                after: Box::new(move || {
                    let pushed = lane.iter().flat_map(|lane| lane.sweep(&broker));
                    (refreshed.into_iter().chain(lease.revoke()).chain(pushed)).collect()
                }),
            })
        })
    }

    /// One holder's standing with the process's proxy, bound here on the first contained spawn.
    pub(crate) fn egress_lease(
        self: &Arc<Self>,
        wall: crate::wall::Wall,
    ) -> Result<crate::egress::Lease, crate::egress::Error> {
        crate::egress::Proxy::shared()?.lease(Arc::clone(self), wall)
    }

    /// Allowed with no ask: seeded, named in config, kept by an earlier "always", or yolo.
    pub(crate) fn egress_allowed(&self, domain: &Domain) -> bool {
        let seeded = SEED.contains(&domain.as_str());
        let named = (self.egress.config.get()).is_some_and(|named| named.contains(domain));
        let canonical = yi_permission::egress_grant(domain).canonical;
        let kept = self
            .lock_rules()
            .decision_for(RuleKind::Command, &canonical)
            == Some(RuleDecision::Allow);
        let yolo = self.mode() == PermissionMode::Yolo;
        let by = match (seeded, named || kept, yolo) {
            (true, _, _) => "seed",
            (_, true, _) => "rule",
            (_, _, true) => "yolo",
            _ => return false,
        };
        self.first_egress(domain, true, Answerer::Other(by.to_owned()));
        true
    }

    /// Blocks until a person answers; a headless session has none, and that is a refusal.
    pub(crate) fn egress_ask(&self, domain: &Domain) -> bool {
        let description = format!(
            "A contained command wants the network at {domain}, which is not seeded or in sandbox.egress"
        );
        let grants = [yi_permission::egress_grant(domain)];
        let ask = PermissionAsk {
            title: "egress",
            description: &description,
            patch: None,
            changes: &[],
            grants: &grants,
            tool_call_id: None,
        };
        let (outcome, _classifier) = self.confirm_judged(&ask, None);
        let allowed = !matches!(outcome, AskOutcome::Reject);
        if let AskOutcome::AllowAlways(_) = outcome {
            self.keep_grant(grants.first(), RuleKind::Command, "", domain.as_str());
        }
        // The ask journaled itself; a later "rule" row would only repeat it.
        if let Ok(mut recorded) = self.egress.recorded.lock() {
            recorded.insert(domain.clone());
        }
        allowed
    }

    /// The ledger holds the first decision per domain, not one row per connection.
    fn first_egress(&self, domain: &Domain, allowed: bool, by: Answerer) {
        let first = (self.egress.recorded.lock()).is_ok_and(|mut seen| seen.insert(domain.clone()));
        if !first {
            return;
        }
        let description = format!("network to {domain}");
        let ask = PermissionAsk {
            title: "egress",
            description: &description,
            patch: None,
            changes: &[],
            grants: &[],
            tool_call_id: None,
        };
        self.settle(&format!("egress-{domain}"), &ask, allowed, by);
    }
}
