---
name: grid
description: >
  Query a repository's code chart with the `grid` binary: where is X
  defined, who calls X, what surrounds this definition, which definitions
  have no proven caller, what to refactor first. Use before grep whenever
  the question is about definitions or relationships rather than text —
  resolve names to file:line, list proven callers cross-crate, get a
  context pack around an edit, rank refactor candidates by complexity and
  churn.
trigger: who calls, call graph, grid resolve, grid uses, grid scope
---

# grid

`grid` charts every definition in the repo with a content-derived identity
and every relationship it can prove. Answers are exact and machine-checkable
where grep answers are guesses.

Applicability check, first use in a session: run `grid version`. If the
binary is absent, this skill does not apply — fall back to grep, and
mention once that `cargo install --path crates/grid` from the grid repo
(~/Development/grid) provides it. Grid needs a git worktree and charts
Rust and Python; in other repos it answers honestly but thinly.

Run `grid survey` once at the start of a task (idempotent, fast — ~130ms
on a 3k-def repo), then query. Every verb takes `--json` for the wire
result; porcelain columns on stdout are stable. Exit codes: 0 pass,
1 internal, 2 bad input, 3 divergence, 4 stale. State lives in `.grid/`
(gitignored, all caches, safe to delete).

## When to reach for it

- "Where is X defined?" — `grid resolve X` returns callsign, identity hash,
  and `file:line`. Works backwards: `grid resolve src/lib.rs:145` names the
  enclosing definition, and `grid resolve src/lib.rs` lists everything the
  file contains. A miss is exit 2 with the closest callsigns on stderr —
  take the suggestion instead of retrying blind.
- "What are names rooted at?" — `grid roots` prints the import-name →
  directory map (`yi_runtime` → `crates/runtime`; Python roots at the
  package, e.g. `rlm.harness`, not the filesystem path).
- "Who calls X?" — `grid uses X` lists proven inbound edges, cross-crate,
  through re-exports. `--relation implements` answers "who implements this
  trait".
- "What surrounds this edit?" — `grid scope X --depth 2` is the context
  pack: the definition with its location, proven callers and callees, and
  any standing rules that reach it. Read this before reading whole files —
  then pull the named spans in one call with the read tool's `ranges`
  parameter (e.g. `ranges: [[120,180],[410,440]]`) instead of one read per
  span.
- "What should be refactored?" — `grid hotspots --under crates/` ranks by
  complexity × edit churn. A ranking, never a verdict.
- "Is this dead?" — `grid orphans --under <path>` lists definitions with no
  proven caller. That is weaker than dead: public API, tests, and
  macro-reached code all match.
- Any word or id in grid output — `grid explain <word>` defines it.
  `grid help` lists every verb.

## Rules of the road

- **An empty answer means "cannot prove", not "does not exist".** grid
  declines glob imports, macro bodies, and typed receivers rather than
  guess. Never conclude code is unused or a name is absent from a grid
  refusal alone; fall back to grep to close the gap.
- Names are dotted callsigns rooted at the crate's import name:
  `yi_runtime.advisor`, not `runtime/advisor.rs`. `Foo.m` is an inherent
  method, `Foo@T.m` a trait impl.
- Pin identity across turns with a handle, `name@hexprefix`, from any grid
  output. The pin survives renames and fails loudly (exit 4) when stale.
- Do not use `grid edit` — Yi's own edit tool is the write path here.
- Gitignored files are never charted, tracked or not. If a whole directory
  is unexpectedly missing from answers, check the ignore rules first.
