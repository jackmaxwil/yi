---
name: yi-dogfood
description: Dogfood any Yi tool the way D171 was — claims ledger, real corpus, census, probes, a played session scored against pre-registered truth, and the replay that goes in the PR
---

# Dogfooding a tool

Run this for a new tool, a new kernel extra, or a new input kind (all six steps), and
step 1 alone for any change to a description, schema or identity sentence. A green suite is
where this starts: every D171 defect sat beside passing tests.

1. **Claims.** List every sentence the model will read about the change — description,
   schema docs, errors, hints that name a remedy — as `claim · check · result`. Derive the
   claim from the thing where you can (the format list from the wheel), test it where you
   cannot, and run every remedy a hint names in the real venv or shell. For each loop a
   user will run, write where the answer lives without the tool: a second format, the
   task's scorer, a second engine. This table is the PR's `## Claims ledger`.
2. **Corpus.** For a tool that reads files, `just dogfood sample <run> <exts> <roots>…` copies
   a stratified sample (kind × size, extremes kept) into `target/dogfood/<run>/in`. For any
   other tool, `just dogfood mine <run> <tool>` takes its real inputs from `~/.yi/sessions`
   (bash only where `kind_for` says read-only). The per-tool corpus and truth table is in
   docs/plans/2026-09-10-dogfood-method-and-mandates.md §2. Add lookalikes by hand: wrong
   extensions, lock files, other encodings, the dependency's own adversarial inputs.
   Copies only; never a path under the tree.
3. **Census.** `just dogfood census <run> <tool> '{"path": "{path}"}'` (or over the mined
   `inputs.jsonl`), then `just dogfood report <run>`. Stateful tools (`edit`, `write`,
   `plan`, `todo`, `ask_user`) skip this step and are covered in play. Read the outliers of every bucket. The bucket you did not
   expect to have members is the bug (RTF sat in "plain text").
4. **Probe.** Every suspicion becomes a minimal probe before it is a finding: N threads on
   one new input, the limit and the limit plus one, one bad part, a content change that keeps
   the timestamp, a cancel, a non-UTF-8 name. Walk the ten classes in
   docs/plans/2026-09-10-dogfood-method-and-mandates.md §3; give each finding its evidence
   and P1 (wrong or unsafe), P2 (costly) or P3 (polish).
5. **Play.** Write `truth.tsv` (question, answer, source) before the first call. Start
   `just dogfood play <run> <cwd>` in the background, read `<run>/tools.md` and nothing in
   the source, and drive it with `just dogfood call <run> <tool> '<json>'`, each call chosen
   from the last output. Execute every hint. Put your answers in `answers.tsv`;
   `just dogfood report <run>` scores them and counts calls, errors and bytes. Triangulate
   a wrong answer across formats, engines and tools before you blame one.
6. **Close.** Each finding: a real-producer fixture (never Yi's output, a user's file or a
   benchmark's answer key), a test seen red under a mutation, the fix. Each P1 from play
   becomes a question in crates/runtime/tests/fixtures/journeys/. Then
   `just dogfood replay <run> <run2>` and paste its table into `## Dogfood`, and
   `just dogfood clean <run>`. Stop when a replay and a census rerun find no new P1 or P2.

The player reaches every tool the session registers, through the same adapters a model's
call passes (rules, wall, broker, sandbox, extension hooks). What it cannot see is the loop
between calls: compaction, steering, retries. A mutation that changes the extras
runs with `--venv <run>/venv`; never delete a `~/.yi/kernel-venv-*` you did not create.
Report only counts, kinds, timings and the one line that shows each defect.
