//! Which forge calls may leave the sandbox: a forge CLI aimed at this project's origin, and a
//! recipe in the home config whose pinned paths match the trunk on origin.

use std::path::Path;

use yi_permission::{Decision, Parsed};
use yi_types::config::HostRecipe;

use super::{AskOutcome, CallOutcome, Containment, PermissionAsk, PermissionBroker};
use crate::lane::hostgit::{git, trunk};
use crate::lane::land::{host_of, owner_repo};

const TARGETS: [&str; 4] = ["-R", "--repo", "--hostname", "--config"];

/// The flag and its value for each flag that aims a forge CLI somewhere, `--repo x` or `--repo=x`.
fn aims(argv: &[String]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (at, word) in argv.iter().enumerate() {
        let (flag, value) = match word.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_owned())),
            None => (word.as_str(), argv.get(at.saturating_add(1)).cloned()),
        };
        if TARGETS.contains(&flag) {
            found.push((flag.to_owned(), value.unwrap_or_default()));
        }
    }
    found
}

/// What a forge command aims at that this origin is not. A `--config` is always elsewhere: it names
/// a login no rule here was written for.
pub(super) fn aimed_elsewhere(argv: &[String], origin: &str) -> Option<String> {
    let program = argv
        .first()
        .map(|word| word.rsplit('/').next().unwrap_or(word))?;
    if !matches!(program, "fgj" | "gh") {
        return None;
    }
    aims(argv).into_iter().find_map(|(flag, value)| {
        let matches_origin = match flag.as_str() {
            "--hostname" => host_of(origin).is_some_and(|host| host.eq_ignore_ascii_case(&value)),
            "--config" => false,
            _ => owner_repo(origin).is_some_and(|repo| {
                let value = value.strip_suffix(".git").unwrap_or(&value);
                value == repo || value.ends_with(&format!("/{repo}"))
            }),
        };
        (!matches_origin).then(|| format!("{flag} {value}"))
    })
}

impl PermissionBroker {
    /// An allow rule is for this project's forge, so a call aimed elsewhere asks whatever it says.
    pub(super) fn forge_gate(&self, command: Option<&str>, decision: Decision) -> Decision {
        let Some(command) =
            command.filter(|command| command.contains("fgj") || command.contains("gh "))
        else {
            return decision;
        };
        if self.mode() == yi_permission::PermissionMode::Yolo
            || matches!(decision, Decision::Deny { .. })
        {
            return decision;
        }
        let origin = git(&self.cwd, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
        let aimed = (yi_permission::command_segments(command).iter())
            .find_map(|argv| aimed_elsewhere(argv, origin.trim()));
        match aimed {
            Some(aimed) => Decision::Ask {
                title: "bash requires permission".to_owned(),
                description: format!(
                    "`{command}` aims at {aimed}, which is not this project's origin"
                ),
                reviewable: false,
            },
            None => decision,
        }
    }

    /// `Some` for one command that starts with a recipe's words. Compounds never match: what
    /// followed the recipe would leave with it.
    pub(super) fn host_recipe(&self, command: Option<&str>) -> Option<CallOutcome> {
        let command = command?;
        if self.mode() == yi_permission::PermissionMode::Yolo || self.sandbox.is_none() {
            return None;
        }
        let recipes = self.host_recipes.get()?;
        let Parsed::Segments(segments) = yi_permission::parse(command) else {
            return None;
        };
        let [argv] = segments.as_slice() else {
            return None;
        };
        let recipe = recipes.iter().find(|recipe| {
            let words: Vec<&str> = recipe.call.split_whitespace().collect();
            !words.is_empty()
                && words.len() <= argv.len()
                && argv.iter().zip(&words).all(|(word, want)| word == want)
        })?;
        let outside = |reason: String| CallOutcome {
            allowed: true,
            reason,
            containment: Containment::Uncontained,
        };
        let Some(reason) = unverified(&self.cwd, recipe) else {
            let call = &recipe.call;
            return Some(outside(format!(
                "`{call}` runs outside the sandbox: its pins match the trunk on origin"
            )));
        };
        let description = format!(
            "`{command}` runs outside the sandbox, but {reason}, so it would run what this tree wrote"
        );
        let ask = PermissionAsk {
            title: "host recipe",
            description: &description,
            patch: None,
            changes: &[],
            grants: &[],
            tool_call_id: None,
        };
        let (outcome, _classifier) = self.confirm_judged(&ask, None);
        Some(match outcome {
            AskOutcome::Reject => self.denied(description),
            _ => outside(format!("`{command}` approved outside the sandbox")),
        })
    }
}

/// Why a recipe's pins do not hold, or `None` when every pin matches the trunk on origin: an empty
/// list pins nothing, and a pin naming no path in the trunk is no pin at all.
fn unverified(cwd: &Path, recipe: &HostRecipe) -> Option<String> {
    if recipe.pin.is_empty() {
        return Some("its pin list is empty".to_owned());
    }
    let base = format!("refs/remotes/origin/{}", trunk(cwd));
    (recipe.pin.iter()).find_map(|pin| {
        let in_trunk = git(cwd, &["ls-tree", "--name-only", &base, "--", pin])
            .is_ok_and(|listed| !listed.trim().is_empty());
        if !in_trunk {
            return Some(format!("{pin} is not in the trunk on origin"));
        }
        let same = git(
            cwd,
            &[
                "diff",
                "--quiet",
                "--no-ext-diff",
                "--no-textconv",
                &base,
                "--",
                pin,
            ],
        )
        .is_ok();
        if !same {
            return Some(format!("{pin} differs from the trunk on origin"));
        }
        let clean = git(cwd, &["status", "--porcelain", "--", pin])
            .is_ok_and(|status| status.trim().is_empty());
        (!clean).then(|| format!("{pin} holds untracked work"))
    })
}
