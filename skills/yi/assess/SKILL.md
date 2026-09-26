---
name: assess
description: >
  Assess, rate, audit or review a whole repository or a large subsystem from
  what it is, not from a re-run of its gates. Use when the user says "rate",
  "assess", "audit", "evaluate", "how good is", "comprehensive analysis",
  "review the codebase". Do NOT use for a diff or a pull request (that is
  the review skill) or for a bug ("why does X fail" is a diagnosis).
trigger: rate it, assess, audit, evaluate, comprehensive analysis, how good is, review the codebase
scope: text
---

# assess

`assess` → todo list → read the record → read the code → claims with evidence → answer

An assessment is a reading task. Its evidence is the code, the docs, the
history and the repository's own gate records. Running the suite or a lint
gate to learn their state is the mistake this skill exists to prevent: the
repository already ran them and wrote the result down, and a test that
fails inside your own sandbox (PermissionDenied, no network, no socket)
says nothing about the code.

## Phase 0: the list

Before the first read, `todo` `init` with these items, one per dimension the
user named plus the three that always apply:

- read the record (architecture entry point, decision log tail, changelog, gate recipe, last merges)
- read the code (three source files chosen from the orientation packet)
- one item per dimension you will score
- write the answer with an evidence column

## Phase 1: the record

Read, in this order, and do not skip one because it looks long:

1. `get_context` once. Note `PARTIAL - k of n layers`: a missing layer is
   absent, not empty; read what the packet names.
2. The architecture entry point: for this repository `docs/ARCHITECTURE.md`
   header (version, status) and the last ten decision rows; elsewhere the
   README's architecture section or `ARCHITECTURE.md` if present.
3. The design document's first section (`docs/YI_DESIGN.md` §1.1 here).
4. `git log --oneline -40` and the changelog head.
5. The gate recipe: the `justfile`, `Makefile`, CI workflow or `package.json`
   scripts. Quote the gate command; do not run it.
6. The last merged pull requests and their CI state where a forge is
   reachable (`fgj pr list --state closed -R owner/name` on Forgejo; `gh pr list
   --state merged --limit 5` on GitHub). That line is the gate record you
   cite instead of a local run.

## Phase 2: the code

From the orientation packet's skeletons and change heat, pick at least:

- one boundary crate or module (the one every other one depends on),
- the hottest module by change heat,
- one test file,
- one guardrail or CI script if the repository has them.

Read each whole (`read` with a large range, once). Note what the code does
that the record does not say, and what the record claims that the code does
not bear out. Those contradictions are the findings that matter most.

## Phase 3: counts, only after reading

A grep count is not a finding. Before writing any number:

- name the scope (`crates/*/src` is production; `tests/` and `#[cfg(test)]`
  modules are not; `vendor/` is someone else's code),
- read the matches, or at least the first page of them,
- write the command beside the number in the answer.

A `#[cfg(test)]` attribute on one line does not exclude the module under it;
a grep over the tree counts vendored files; `unsafe ` matches the English
word in a comment. Say what was counted.

## Phase 4: the answer

The assessment shape from the Voice section: as long as the evidence
requires; the dimensions the user named or the ones the evidence supports;
every claim cites what was read (a file, a decision row, a commit, a gate
record); every number carries its command and its scope; contradictions
first. A table for the scores is right because scores are tabular; the
argument stays in prose.

Then step every todo to done with its evidence, and end. Do not close with
an offer.

## Do NOT

- Run the test suite, clippy, a build, or a benchmark to learn their state.
- Report a count you did not read.
- Score a dimension the evidence does not reach; say the evidence was not
  read and why.
- Blame the codebase for a failure your sandbox produced.
