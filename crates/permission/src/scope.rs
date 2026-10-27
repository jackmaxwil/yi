use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use yi_types::permission::RuleKind;

use crate::catastrophic::{CatastrophicContext, absolute, lexical_normalize, resolve_links};
use crate::decide::ToolCall;
use crate::rules::{Grant, SessionRules, identity, tagged};

const SCOPE: &str = "write-scope";

fn compile(pattern: &str) -> Result<Glob, String> {
    if pattern.is_empty() || pattern.starts_with('/') {
        return Err(format!(
            "{pattern:?} is not relative to the working directory"
        ));
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| format!("invalid scope {pattern:?}: {error}"))
}

/// The brief of a run: every later write inside the working tree must fall under one scope
/// (a glob, relative to the tree) or be refused. A scope is kept as a command rule.
pub fn write_scope(pattern: &str) -> Result<Grant, String> {
    compile(pattern)?;
    Ok(Grant {
        kind: RuleKind::Command,
        canonical: identity(&[SCOPE, pattern]),
        label: format!("writes only under {pattern}"),
    })
}

/// Why this call may not run: it writes a path inside the working tree that no scope names.
/// A path is named when it, or a directory above it, matches. Outside the tree is the sandbox's.
pub(crate) fn outside(
    rules: &SessionRules,
    call: &ToolCall<'_>,
    context: &CatastrophicContext,
) -> Option<String> {
    let patterns: Vec<&str> = (rules.state().rules.iter())
        .filter_map(|rule| tagged(rule, SCOPE))
        .collect();
    if patterns.is_empty() || (call.reads_only && !call.irreversible) {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for glob in patterns.iter().filter_map(|pattern| compile(pattern).ok()) {
        builder.add(glob);
    }
    // A scope that cannot compile names nothing, so an empty set refuses every write.
    let set = builder.build().unwrap_or_else(|_| GlobSet::empty());
    let root = resolve_links(&lexical_normalize(context.working_dir.as_deref()?));
    let redirected: Vec<_> = (call.command.into_iter())
        .flat_map(|command| crate::safety::redirect_targets(command, true))
        .collect();
    if let Some(target) = redirected.iter().find(|target| !target.directory_known) {
        return Some(format!(
            "{} follows a `cd` whose directory cannot be read statically, so it cannot be shown \
             to be inside this run's write scope ({}). This is denied in every mode.",
            target.path,
            patterns.join(", ")
        ));
    }
    let redirected = redirected
        .into_iter()
        .map(|target| absolute(Path::new(&target.path), context));
    // A `grep` that applies a rewrite and names no path walks from the working directory.
    let walks = call.tool_name == "grep" && call.targets.is_empty();
    let written = (call.targets.iter())
        .map(|target| absolute(target, context))
        .chain(walks.then(|| root.clone()))
        .chain(redirected);
    for path in written {
        let real = resolve_links(&path);
        let Ok(relative) = real.strip_prefix(&root) else {
            continue;
        };
        let named = (relative.ancestors()).any(|dir| {
            !dir.as_os_str().is_empty() && (set.is_match(dir) || set.is_match(dir.join("")))
        });
        if !named {
            let shown = relative.to_string_lossy();
            return Some(format!(
                "{} is outside this run's write scope ({}). This is denied in every mode; leave it \
                 unchanged and say in your answer what the fix needs there.",
                if shown.is_empty() {
                    "the working directory"
                } else {
                    shown.as_ref()
                },
                patterns.join(", ")
            ));
        }
    }
    None
}
