---
name: grid
description: Query the repository's code chart with the `grid` binary — resolve names to file:line, find proven callers, get a context pack around a definition, rank refactor candidates. Use before grep when the question is about definitions or relationships, not text.
---

# grid

`grid` charts every definition in the repo with a content-derived identity
and every relationship it can prove. Answers are exact and machine-checkable
where grep answers are guesses. It is a single binary on PATH; every verb
takes `--json` for the wire result, and porcelain columns on stdout are
stable. Exit codes: 0 pass, 1 internal, 2 bad input, 3 divergence, 4 stale.

Run `grid survey` once at the start of a task (idempotent, ~150ms on this
repo), then query. State lives in `.grid/` (gitignored, safe to delete).

## When to reach for it

- "Where is X defined?" — `grid resolve X` returns callsign, identity hash,
  and `file:line`. Works backwards: `grid resolve src/lib.rs:145` names the
  enclosing definition.
- "Who calls X?" — `grid uses X` lists proven inbound edges, cross-crate,
  through re-exports. `--relation implements` answers "who implements this
  trait".
- "What surrounds this edit?" — `grid scope X --depth 2` is the context
  pack: the definition with its location, proven callers and callees, and
  any standing rules that reach it. Read this before reading whole files.
- "What should be refactored?" — `grid hotspots --under crates/` ranks by
  complexity × edit churn. A ranking, never a verdict.
- "Is this dead?" — `grid orphans --under <path>` lists definitions with no
  proven caller. That is weaker than dead: public API, tests, and
  macro-reached code all match.
- Any word or id in grid output — `grid explain <word>` defines it.

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
