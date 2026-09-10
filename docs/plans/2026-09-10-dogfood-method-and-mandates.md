# Dogfooding Yi's tools: the method, and the checks it mandates

```
status:  PROPOSED (#376)
date:    2026-09-10
inputs:  docs/plans/2026-09-09-anydoc-and-pdf-inspector.md §12 (as built, round
         two, round three) · PR #369, PR #371, issue #368 · this tree at 2d1d7aa:
         crates/tools/src/{document.rs,ipython.rs,hashline/tool.rs,grep.rs,lib.rs},
         crates/kernel/src/bootstrap.rs, crates/runtime/src/{wiring.rs,kernel.rs,
         auto_review.rs,provider.rs,lib.rs,session.rs,tools.rs}, crates/cli/src/main.rs, crates/runtime/tests/{documents.rs,
         behavior.rs,request_budget.rs,prompt_drift.rs,kernel_lane.rs},
         crates/runtime/tests/fixtures/{documents,behavior}/ ·
         scripts/{pr_body.py,forge_pr.py}, scripts/guardrails/{check_pr_metadata.py,
         check_request_budget.py,check_test_tiers.py,check_test_size.py,
         check_guardrails.sh,_common.py} · .forgejo/workflows/{pr.yml,postmerge.yml}
         · justfile · .ruler/{010,040,045,080,090,095,097,100}.md and the yi-forge,
         yi-refute and yi-tui-verify skills · evals/orient_census.py · the two scratch
         harnesses the D171 session ran (never committed; their source reached this
         document through the brief that commissioned it) · open PRs #365, #372, #374,
         #375 · measurements run for this document on this machine, 2026-09-10
ruling:  this covers every tool a session registers; D171 is the worked
         example, not the boundary. The method is a skill and one dev verb,
         not a gate. Dogfooding needs a
         reader of outputs, so it cannot run in CI, and no gate may pretend it
         did. What a gate can do deterministically is notice that the surface
         a model reads has changed — a lock over the tool table a session
         registers — and refuse a PR body that changes that surface without a
         claims ledger, or adds to it without a neighbour matrix and a dogfood
         table. The play session's findings are pinned as a tier-2 journey over
         the repo's own fixtures, scored line by line against truth that did not
         come from Yi. Nothing here runs a model, spends money, or judges prose.
```

## 0. Conflicts with the tree (recorded; the tree wins)

1. **`crates/runtime/tests/documents.rs` holds 28 `#[test]` functions, not 29.** One
   (`a_document_whose_name_is_not_utf8_converts`, line 506) is `#[cfg(target_os = "linux")]`,
   so macOS runs 27. Measured: `cargo test -p yi-runtime --test documents` → `27 passed` in
   1.33 s on this machine with a warm venv.
2. **The decision log lives in `docs/ARCHITECTURE.md` (`## Decision log`, D171 at line
   145), not in YI_DESIGN §14.6** as the D171 plan's §11 says. `scripts/adr.py` reads
   ARCHITECTURE.md.
3. **D172 is not free.** Open PR #372 adds `| D172 |` at 0.208.0; #374 adds `| D169 |` at
   0.209.0; #365 adds `| D170 |` at 0.207.0, a version main already took with D171, so it
   has to move. #285 is closed. At the time of writing the next unclaimed row is **D173**
   and the next unclaimed version **0.210.0** — re-read both at implementation (§15).
4. **`scripts/pr_body.py` prints no `## Seen red` section**, although
   `.github/PULL_REQUEST_TEMPLATE.md` has one and 080/090 require it. The body a PR is
   opened with comes from pr_body.py, so the section is written by memory or not at all.
5. **The request budget measures a tool table no session registers.**
   `request_budget.rs::tool_defs` is `yi_tools::builtin_tools()` (no documents, so no format
   clause) plus `plan` and `todo`. A real session also registers `ipython`
   (`wiring.rs:554`) and `ask_user` (`auto_review.rs:265`/`270`), and its `read` carries the
   document clause. Counted from the source strings: ipython's description is 830 bytes and
   its `code` parameter text 325; the clause, with this machine's recorded formats, is 451;
   `ask_user`'s description is about 370. So `tools: 16748` understates what every request
   pays by at least 1.9 KB, before the two schemas.
6. **`forge_pr.py compose_body` appends pr_body.py's whole output after the author's
   prose**, so a `--body` that already has the template's sections gets every counted
   section twice. The D171 landing worked around it by opening with no body and editing
   it afterwards.
7. **Whether a body edit re-runs the `title` job is not written down anywhere.** `pr.yml`
   says `on: pull_request:` with no `types:`, so it depends on the forge's defaults. §13,
   question 6 records one observation.

## 1. What D171 showed, measured

Every unit test and fixture was green at every stage. Every serious defect was found by
running real files through the real code path, and the worst three only turned up in a
session played as Yi, asking questions whose answers could be checked:

| round | what was run | what only it found |
|---|---|---|
| 1. census | 141 of the owner's documents, stratified by kind and size with the extremes kept (a 502 MB PDF, a 48 MB RTF), each read cold then warm through `builtin_tools_with` | RTF never converted: it is 7-bit text, so the non-UTF-8 trigger never fired. The "plain text" bucket held all 10 RTFs, starting `1:{\rtf1\ansi\deff3\adeflang1025`. A PDF with one image-only page was refused whole |
| 2. review | 47 gaps in ten classes, each confirmed by a minimal probe first | 3 threads × 20 rounds on one new `.docx`: 40 of 60 calls failed `cannot keep the copy: No such file or directory` (one shared staging name) |
| 3. play | 38 calls in one stateful session (the real tools plus the ipython kernel), each call chosen from the last output, on two terminal-bench tasks and a coursework folder | `read find="Operating Systems"` → `\|Class: 3677-02-Lecture CPSC 380 Operating Systems\|…\|Tuesday Thursday 11:30AM\|Hashinger 327\|`, while the xlsx says Monday Wednesday 5:30PM, Keck Center 156. Answer-key cells like `(5, -10)` were split in two, so a naive parse scored 47/48. `claimed: in ipython, pandas reads a spreadsheet of this size` → `ImportError: Missing optional dependency 'openpyxl'`. `grep "Faraday pail"` → `No matches found` plus `[11 binary files skipped — bash: rg -a for those]`, while a `.docx` held the phrase four times. A 57 KB first read |

After the fixes, the same session replayed: 48/48 on the answer keys, the right schedule
answer, pandas working first time, `grep` finding the phrase, a 16 KB first read with the
outline first, and `find="Faraday's Law"` going from a miss (then 1.3 s and 10 KB of refs) to
26 ms.

Three things follow, and the rest of this document is built on them:

- **The expensive defects were claims.** A description, a hint or a table row said
  something the code did not do. None of them was a crash. A claim can be listed before
  any code runs, and each one can be given the check that would disprove it.
- **Ground truth was always somewhere else**: the same data in another format, a
  benchmark's own scorer, a second engine. The play session found wrong answers because it
  had a way to know they were wrong.
- **What made it work was a harness, not effort.** Two scratch test files let one person
  drive the real tool set as a model would. Uncommitted, they are gone; this proposal
  checks them in (§4).

## 2. The method, final form

Six phases, A to F. There are three changes from the draft the D171 session wrote down,
each explained at its step:

- truth is written down before playing (step 3);
- the play path becomes a checked-in replay (step 16);
- there is a stopping rule (step 18).

**Scope: every tool a session registers.** That is `read`, `edit`, `write`, `grep`, `bash`
and `get_context` (`yi_tools::builtin_tools_with`); `ipython`, `ask_user`, `plan` and
`todo` (added by `attach_runtime`); every exec tool discovered under `.yi/tools`; and every
kernel extra a hint can name. D171 is the worked example because it is the one that was
run. Nothing below is specific to documents except where a document is the example. From
tool to tool, only two things change: where the corpus comes from and where the truth lives.

| tool | corpus (step 4) | truth (step 2) | census or play |
|---|---|---|---|
| `read`, `grep` | files from the owner's folders and checkouts, stratified by kind × size | the same content in another format; `rg`; a second engine | both |
| `get_context` | the owner's checkouts | the `grid` CLI and `rg`, for what it says is where | both |
| `bash` | commands mined from `~/.yi/sessions` that `BashTool::kind_for` classes as read-only, run in a scratch copy of the checkout | the same command in a plain shell, unreduced | both |
| `ipython` | cells mined from `~/.yi/sessions` | the venv's `python3` running the same cell | both |
| `edit`, `write` | real files copied from checkouts, edited in play | `git diff` of the copy; the file's bytes afterwards | play only: an edit depends on the read before it |
| `plan`, `todo`, `ask_user` | task sequences taken from real sessions | the store state each call claims to leave behind | play only: each call depends on session state |
| exec tools | each tool's own `--schema` | running the script directly on the same stdin | both |

**A. Frame the claims.**

1. **Claims ledger.** Every sentence the model sees is a claim: the tool description,
   the schema docs, error text, and any hint that names a remedy. Give each claim the check
   that would prove it false. Derive the claim from the thing itself where you can: D171
   derives the format list from the wheel, and `prompt_drift.rs` pins it both ways. Test it
   where you cannot. Write the ledger as a table: claim · check · result. Two rows of
   ipython's description as it stands on main, ledgered for this document:

   | claim | check today | result |
   |---|---|---|
   | "pandas (with openpyxl) reads spreadsheets" | `a_large_sheet_points_at_a_pandas_route_that_works` runs `pandas.read_excel` in the converter's interpreter | holds, but the route the hint names, the session's `ipython` tool, is untested (§7 question 4 closes it) |
   | "`anydoc` and `pdf_inspector` (`extract_text`) read documents" | the bootstrap's import probe (`DEFAULT_RLM_EXTRA_IMPORT_NAMES`) | only the import is checked. `pdf_inspector.extract_text("single.pdf")`, run by hand for this document, returns the page's text; no test calls it |
2. **Truth sources.** For each loop a user will run, say where its answer lives without
   the tool: the same content in a second format, a benchmark's scorer, a reference
   engine (`pdftotext -layout` reads `layout.pdf` correctly, which is how its fixture's
   truth can be checked by something that is not Yi).
3. **Pre-register** (new). Before the first call of phase E, write each question, its
   expected answer and its truth source into `truth.tsv`. A score written after reading the
   output bends towards the output. Round three worked because its answers were checkable;
   writing them down first makes the checking mechanical.

**B. Build a real corpus.**

4. Sample from where the tool will run: the owner's folders, checkouts, `~/.yi/sessions`,
   `ref/benchmarks`. Stratify by kind × size, keep the extremes, and add **lookalikes**:
   wrong extensions, lock files, other encodings, and the dependency's own adversarial test
   inputs.
5. Work only on **copies**, under a scratch HOME (§4.2). Delete them when done: they are
   personal files. They never enter the tree, and no PR quotes more than the one line that
   shows a defect.

**C. Census.**

6. Drive the production constructors (`builtin_tools_with`, `yi_runtime::documents`, the
   real venv), not the component under test.
7. Bucket every outcome, with timings, then **read the outliers of each bucket**. The
   bucket you did not expect to have members is where the bug is: in round one that was
   "plain text", which held all 10 RTFs.

**D. Probe and review.**

8. Turn every suspicion into a minimal probe **before** listing it: threads, a limit
   and the limit plus one, encodings, and a content change that keeps the timestamp.
9. Walk the ten classes of §3 in order. Give each finding its evidence and a priority:
   **P1** wrong or unsafe, **P2** costly, **P3** polish.

**E. Play the agent.**

10. Hold one stateful session (`just dogfood play`, §4.4) and read **no source** while
    playing. Read `tools.md` first, since it is what the model reads.
11. Use tasks whose answers can be checked (step 3). Choose each call from the previous
    output only.
12. Score the answers against `truth.tsv`. Count calls, dead ends and result bytes.
    **Execute every hint** the tools give.
13. **Triangulate** across formats, engines and tools to find whose fault a defect is:
    the library's own Markdown made the same table error as `read`, but its reading-order
    text did not.

**F. Close the loop.**

14. Each finding becomes a real-producer fixture (never Yi's output, never a user's file,
    never a benchmark's answer key), then a test seen red under a mutation, then a fix.
15. Replay the same session (`just dogfood replay`) and put the before/after table in the
    PR's `## Dogfood` section.
16. **Pin the path** (new). Each P1 found in play becomes a question in a tier-2 journey
    cassette over repo fixtures (§7). A play session is gone once it ends; a cassette
    makes sure its path still works on every merge.
17. Record the lessons in the plan's "As built" section.
18. **Stop** (new) when a replay and a census rerun find no new P1 or P2, and every
    bucket's outliers have been read. Without a stopping rule, dogfooding is either skipped
    or never finished.

## 3. The ten classes, and what each one buys

"Mandated" means required by this proposal. The rest are the review questions the skill
asks.

| class (D171 case) | design-time review | dev-time test (T0) | pre-merge | mandated |
|---|---|---|---|---|
| 1. wrong answers that look right (table cells misattributed) | how could each transform be confidently wrong, and what reference checks it | the same content in two formats gives the same facts (`brief.docx` / `brief.rtf`); a row assertion, never a substring over the output | journey question scored by a row assertion (§7) | 080 rule B3; journey |
| 2. inputs that never reach the path (RTF) | routing table: every advertised input, its path, its lookalikes | kind × encoding × wrong extension × empty × size extremes | census over repo fixtures (the journey's workspace) | Claims ledger names the route of each advertised input |
| 3. all-or-nothing failures (one page refuses a PDF) | partial-failure policy per input | a fixture with exactly one bad part; every limit at the limit and the limit plus one | — | 080 rule B2 |
| 4. races and stale state (shared staging name) | state inventory: every cache, temp file, key and invalidation | N threads on one new input; a content change that keeps the timestamp; tamper; cancel; timeout | — | 080 rule B4 |
| 5. safety gaps (write over a `.docx`) | write-path matrix: every writing tool × every new kind | refusals, including on never-read files | — | neighbour matrix (§6) |
| 6. advice that does not work (pandas without openpyxl) | hint audit in the claims ledger | run the remedy in the real environment | journey follows the hint through the session kernel | 080 rule B1; journey |
| 7. neighbour blindness (grep, glob) | neighbour matrix: read, grep, glob, edit, write, bash hints, kernel, TUI, prompts | one test per changed cell | journey greps through a document | neighbour matrix (§6) |
| 8. noise and context cost (57 KB first read) | budget: bytes per call, first look, repeats | a first-look ceiling; no raw markup; the request budget | read the replay transcript as the model would | the widened request budget (§5) |
| 9. awkward to use (curly quotes) | parameters against how models type | near-miss tests | — | review only |
| 10. speed (a spawn per binary) | cost model: cold, warm, negative path, largest input | the negative path observed not to spawn (an artifact or counter, never a timing) | — | review only |

**Discoverability** cuts across all ten. At design time it is the claims ledger. At dev
time it is a claim derived and tested in the `prompt_drift` style. Pre-merge, the
fresh-agent eval is live, paid and run by a person, and is **not** mandated (§12).

The classes are not about documents. Two incidents on record from outside D171 fall into
them:
- `identity.md` described `grep` as literal-only for weeks after it became a regex search.
  That is class 1 in a prompt, and it is why `prompt_drift.rs` exists.
- One landing shipped seven silent caps across glob, grep and `find`. A view that looks
  whole and is not is class 1 again, and it is why `045-loud-caps.md` exists.

## 4. The harnesses

### 4.1 Where they live: an example and a script, behind `just dogfood`

Three places were weighed:

- **A test file gated on an environment variable**, as the D171 session ran them.
  Rejected. With the variable unset, the test returns `Ok(())` and counts as a pass in
  every `just check`, which is a "the code ran" test as 080 defines it. It cannot use
  `#[ignore]` either: `check_test_tiers.py` holds every `#[ignore]` to the tier-2 marker,
  and `just journeys` would then select a session that waits two hours for a player.
- **A `yi dogfood` subcommand.** Rejected. It would put dev tooling in the shipped binary,
  whose size is budgeted, and add a top-level surface that one-in-one-out would have to
  pay for.
- **`crates/runtime/examples/dogfood.rs` plus `scripts/dogfood.py`, behind a
  `just dogfood` recipe.** Admitted.
  - The example links the real crates, the way the scratch test files did, and it is
    compiled on every PR because `just lint` runs `cargo clippy --workspace --all-targets
    -- -D warnings`, and `--all-targets` includes examples. Nothing runs it in CI.
  - The script does the parts that need no Rust: sampling, mining, issuing calls,
    reporting, replaying, cleaning.
  - The recipe is one line, `dogfood *args: python3 scripts/dogfood.py "$@"`, the same
    shape as `pr *args`.

**The ratchets.** The src-only gates (file size 1,200, function size 150, comments,
panics, duplication, env surface) read `crates/*/src/**` through `_common.src_files()`, so
they do not apply to an example. Neither does `check_test_size.py`, which counts
`crates/*/tests/**/*.rs`. That would leave the harness unpriced, and unpriced code is how
bloat gets in. So `check_test_size.py` counts `crates/*/examples/**/*.rs` beside
`crates/*/tests/**/*.rs`, and is re-seeded with `--update` in its own `Ratchet:` commit. That also picks up the one
existing example, `crates/mcp-cli/examples/reference_server.rs`. The harness is priced as
test code, which is what it is. Clippy's `-D warnings` and the workspace lints apply to it
as they do to any target.

**No environment variable is added.** The example takes its arguments on argv, and the
script sets only `HOME` and the existing `YI_KERNEL_VENV`, which is already a row in
`env_vars.json`.

### 4.2 Isolation and privacy

The D171 harnesses passed the real HOME to `KernelService`, which points the kernel's
harness state at `~/.yi/harness` (`RLM_GLOBAL_HARNESS_STATE_DIR`, `kernel.rs:408-416`;
the directory is created when harness state is first saved). This design isolates it:

- **Every run directory is `target/dogfood/<run>/`.** `target/` is gitignored, so
  `check_blob_size` (which lists `git ls-files -co --exclude-standard`) never sees a
  copy, and nothing lands in `/tmp`, which on the home server is RAM.
- `dogfood.py` refuses any `<run>` that is absolute or contains `..`. This is the
  `tui-proof` incident rule: an argument that gets `rm -rf`'d is checked first.
- **HOME is scratch and the venv is borrowed.** The script first runs the built example
  as `dogfood venv` under the real HOME. That calls `ensure_kernel_python` and prints
  `kernel_venv_dir(&home)`, which is the venv this build would use anyway.
- Every later run of the example gets `HOME=<run>/home` and `YI_KERNEL_VENV=<that dir>`.
  `kernel_venv_dir` and `document_converter` both honour `YI_KERNEL_VENV`
  (`bootstrap.rs:311`, `:659`), so the harness state, the copy cache and the venv's name
  all stay off the real HOME. The borrowed venv itself does not: the readiness check
  rewrites its `.bootstrap-version` when the probe hash changes (`bootstrap.rs:797-799`),
  and imports can write bytecode caches into it. That is what any session on this build
  would do to the same venv.
- **The session's own directories are scratch too.** Every path the player hands
  `RuntimeWiring` (`rlm_dir`, `sessions_dir`, `plans_dir`) points inside `<run>`, and no
  lane is claimed, so the player writes no session file, plan or lane where the owner's
  sessions live.
- The example is run as `target/debug/examples/dogfood`, never through `cargo run`. With
  a changed HOME, cargo and rustup would lose `~/.cargo` and `~/.rustup`.
- **A mutation that changes the extras** is run with `--venv <run>/venv`. The throwaway
  venv is then built inside the run and deleted with it. The skill forbids deleting any
  `~/.yi/kernel-venv-*` you did not create; read its `.bootstrap-version` first, because
  another session on another commit may be using it (D171 left venv pruning undone for
  exactly this reason).
- **A drift check, reported and never enforced.** Before and after each run, the script
  lists `~/.yi/converted`, `~/.yi/harness`, the `~/.yi/kernel-venv-*` directory names and
  the borrowed venv's `.bootstrap-version`, and prints any difference. A concurrent session may write there legitimately, so it
  refuses nothing. This carries over the lesson that Yi's tests touch the real HOME, and
  the way to know is to list it before and after.
- **Copies are deleted by `just dogfood clean <run>`.** The skill makes it the last step
  before a PR is opened.
- The census summary prints counts, bucket labels made of detail key names, and timings.
  No file names and no content. This is the rule `evals/orient_census.py` already follows,
  and it is what makes the summary safe to paste into a PR. Names and outputs stay in the
  run directory.

### 4.3 The census, for any tool

`just dogfood sample <run> <ext,…> <root>…` walks the roots and buckets files by extension
× size decile. It keeps the smallest and largest in each extension and up to `--per-cell`
(default 3) per cell, copies them to `<run>/in/`, and writes `manifest.tsv`
(`n<TAB>copy<TAB>bytes<TAB>ext`). The original path is never written into the run.

`just dogfood mine <run> <tool> [--per-cell N]` builds the corpus for a tool whose input is
not a file:
- It reads `~/.yi/sessions/*.jsonl` for calls to `<tool>`, drops duplicate inputs,
  stratifies by result size and `is_error`, and writes `inputs.jsonl`, one input per line.
- Session files hold the owner's prompts and outputs, so the mined inputs stay in the run
  directory under the same rules as copies.
- A `bash` input is kept only when the tool's own `kind_for` classes it as `ToolKind::Read`
  (`builtins.rs:449`, the permission layer's read-only verbs, called rather than copied),
  and it runs in a scratch copy of the checkout. A mined `rm` never runs.

`just dogfood census <run> <tool> '<template>'` runs any tool in the player's table (§4.4),
once cold and once warm, on each manifest row, or on each line of `inputs.jsonl` when the
run has one. The template is the tool's input JSON with
`"{path}"` where the copy's path goes:

- `'{"path": "{path}"}'` for `read`;
- `'{"pattern": "the", "path": "{path}"}'` for `grep`;
- `'{"command": "wc -l {path}"}'` for `bash`.

Each row of `results.tsv` holds n, `is_error`, cold ms, warm ms, bytes, lines and the
compact `details`. Each full output goes to `out/<n>.txt`. The **bucket** is `ok` or
`error` plus the `details` keys that are *not* present in every row. That is generic, and
for `read` it reproduces round one's split: `converted` appears only on converted reads,
and a refusal's reason key only on refusals. `just dogfood report <run>` prints per bucket
the count, p50/max of cold ms, warm ms and bytes, and the manifest numbers of the three
slowest and three largest rows. Those are the outliers to read.

**How a tool author plugs a new tool in:** they don't need to. The player takes its table
from the session wiring the CLI uses (§4.4), so a tool the session registers is in the
census and in play the moment it is registered. A stateful tool (`edit`, `write`, `plan`,
`todo`, `ask_user`) is dogfooded in play only, because each call depends on the one before
it.

### 4.4 The player

`just dogfood play <run> <cwd>` runs in the background. `dogfood play` builds a session the
way the CLI does and plays against every tool that session registers. The D171 harness
held only `builtin_tools_with` plus `ipython_tool`; this one reaches all of them:

- **One tool list.** `yi_runtime::session_tools(freeform_grammar, documents, exec_dir)` is
  new. It is the closure `crates/cli/src/main.rs:504-511` passes as `RuntimeWiring.tools`
  (the builtins with documents, plus the `.yi/tools` exec tools), moved into yi-runtime so
  that the CLI, the player and the surface lock (§5) all call it.
- **The rest comes from `attach_runtime`,** exactly as for the CLI: `ipython`, `ask_user`,
  `plan`, `todo`. The player reads the result back through a new three-line
  `AgentSession::tools()` getter (the field is private, `session.rs:112`).
- **Every call goes through a model's path.** The player calls each `AgentTool::execute`,
  so it passes the same `ToolAdapter` a model's call does: user rules, the wall, the
  permission broker and its sandbox, and the extension hooks (`tools.rs:186-260`).
- **The broker is built as `build_session` builds it** (`main.rs:469-477`), in the
  permission mode given by `--mode`. Headless `yi ask` passes no asker (`main.rs:720-721`),
  and by default the player does the same, so a call that would ask is decided as `yi ask`
  decides it. With `--ask`, each question goes to `a/NNN.ask` instead and waits for the
  player's answer, so a permission prompt is dogfooded like any other output.
- **The kernel is the session's own,** with `kernel_prewarm` off and the isolation of §4.2.

- Before the first call it writes `tools.md`: each registered tool's `definition()` (name,
  description, JSON schema), then `identity_fragment()` and `doctrine_fragment()`. The player reads what a
  model reads, parameter docs included, which the scratch version left out.
- `just dogfood call <run> <tool> '<json>' [lines]` replaces `call.sh`. It writes
  `q/NNN.json` from a counter file rather than `ls | grep -c` (a partial write counted as a
  call there), waits for `a/NNN.txt` and prints its head.
- Each answer is also appended to `session.jsonl` as `{n, tool, input, is_error, ms,
  bytes, details}`. That file is where `report` counts calls, errors, result bytes and the
  largest result, and where `replay` reads its inputs.
- `just dogfood stop <run>` touches `q/stop`. The two-hour idle exit stays.
- `just dogfood replay <run> <new-run>` feeds the same inputs, in order, to a fresh
  session over the same `cwd` and prints the before/after table per call: tool,
  `is_error`, bytes, ms. With `truth.tsv` and an `answers.tsv` in each run, it also prints
  the score before and after. That table is what goes in the PR's `## Dogfood` section.

### 4.5 What the harness cannot see, and the trigger to fix it

It drives tools, not a loop. Because it uses the session's own adapters (§4.4), it now
meets the user rules, the wall, the permission broker and its sandbox, and the extension
hooks, all of which the D171 harness bypassed. What it still lacks is the provider loop:
nothing the loop does between calls (compaction, steering, retries), and no system prompt
beyond what `tools.md` shows. Every D171 finding was at tool level, so this is enough for
the method today.

A loop-level player needs the provider to wait for the player's next message.
`AgentSession::new` takes a concrete `Arc<ProviderStream>` (`session.rs:135`), and
`FauxProvider` answers from a queue that ends the turn when it is empty. So a loop-level
player costs a product change: a delegate on `ProviderStream` that asks an
`Arc<dyn StreamFn>` for the next message, about 25 src lines. Scaffolding ahead of need is
forbidden (100-never). The **trigger**, stated in advance: the first defect found in a
live session that the tool-level player structurally could not have seen, recorded in its
fix's PR. Until then the loop is covered by the existing journeys (`crates/cli/tests/
journeys.rs`) and by live runs a person makes.

## 5. The tool-surface lock

A PR-body gate has to know, without guessing, when the surface a model reads has changed.
Two triggers were weighed:

- **Diff paths**: the ten files with `fn description(&self)`, plus `hashline/prompt.md` and
  `prompts/identity.md`. Rejected. Any bug fix in `grep.rs` would trip it, so authors would
  learn to paste an empty ledger. And a description built from a `const` in another file,
  or from the wheel's format list, would escape it.
- **The rendered surface itself**, locked. Admitted. It is the `schemas.lock` pattern
  applied to what the model reads, and it notices the change wherever the bytes came from.

**The table is the session's table.** `request_budget.rs::tool_defs` changes to the tools a
real session registers. The preferred build is the player's: `attach_runtime` over
`session_tools(false, Some(Documents::fixed(…)), None)`, then the definitions from
`session.tools()`. That makes the lock the registered table by construction, including any
tool `attach_runtime` gains later. If `attach_runtime` cannot run in a T0 test (it would
need no lane and no store, and that is not verified, §13), the fallback is this hand-built
list, built without a kernel, a provider or a broker:

- `yi_tools::builtin_tools_with(false, Some(Documents::fixed(home, Converter { python,
  formats })))`, where `formats` is a constant copy of a recorded `documentFormats` list
  turned into the `Vec<String>` that `Converter` holds, so the document clause is in the table on every machine,
  including one with no venv;
- `yi_tools::IpythonTool { bridge }` over a five-line `KernelBridge` that always refuses;
- `yi_runtime::auto_review::AskUserTool::new(None)`;
- `plan` and `todo`, as today.

The budget's `tools` and `total` rise by what sessions already pay (§0 item 5). That rise
is a measurement correction, not new bytes on the wire, and it lands as its own `Ratchet:`
commit.

**The lock.** The same test prints one line per key:

```
TOOL_SURFACE {"key": "tool:read", "text": "<description>\n<schema as JSON>"}
TOOL_SURFACE {"key": "extra:openpyxl", "text": "openpyxl"}
TOOL_SURFACE {"key": "prompt:identity", "text": "<identity_fragment()>"}
```

- There is one `tool:<name>` per registered tool.
- There is one `extra:<package>` per `DEFAULT_RLM_EXTRA_UV_ARGS` entry, keyed by the
  package name with the version spec stripped. A new wheel is a new capability that hints
  can name.
- `prompt:identity` is the fragment that tells the model what each tool is for; D171
  edited it to name the capability.
- `serde_json` has `preserve_order` in this workspace, so a schema prints the same way
  every time.

`check_request_budget.py` already runs this test once. It gains the lock: it sha256-hashes
each `text` and compares the result with `scripts/guardrails/baselines/tool_surface.json`
(`{key: hash}`). On a mismatch it fails with:

```
tool surface changed: changed tool:ipython; added extra:foo; removed —
  rerun check_request_budget.py --update in its own Ratchet commit;
  the PR body then owes the sections check_pr_metadata.py names
```

Its `--update` rewrites both baselines. The lock is equality, not shrink-only: a removed
key is a surface change like any other and needs `--update`, but the PR gate asks nothing
of a removal (§6). The new baseline is seeded in the commit that adds its reader, which is
the carve-out 040 grants, and `check_orphans.py` finds its reader in the same script.
There is no new `cargo` invocation.

`doctrine` and `mode` are left out of the lock. They tell the model how to work, not what
a tool does, and locking them would trip the gate on every prompt edit (§13, question 1).

## 6. The PR gate

`check_pr_metadata.py` gains one pure function, and `forge_pr.py check_problems` calls it,
so `just pr open` refuses locally before the forge does:

```
surface_problems(body, added, changed) -> [str]
```

- **Trigger.** `measure()` loads `tool_surface.json` at the merge base (`git show
  base:…`, as it already does for the changelog) and from the checked-out tree, which is
  HEAD in CI. Locally, through `forge_pr.py check_problems`, uncommitted edits count too,
  as they already do for the changelog.
  - `added` = keys only in the tree.
  - `changed` = keys in both with different hashes.
  - A lock that is missing at base means the lock itself is being seeded: no requirement.
    Without this, the seeding PR would owe a dogfood table for every existing tool.
- **Requirement.**
  - `changed` non-empty → `## Claims ledger`.
  - `added` non-empty → `## Claims ledger`, `## Neighbour matrix` and `## Dogfood`.
  - A removal-only delta asks for nothing: removing a tool or a claim makes no new claim.
- **Shape, not content.** A required section passes when:
  - the `## <title>` heading is present at level two, matched case-insensitively;
  - the text under it, up to the next `## ` and with `<!-- … -->` stripped, holds a
    Markdown table: a `|…|` row, a separator row of `-`, `:` and `|`, and at least one data
    row after the separator.

  Nothing reads the cells. Reviewers read them.
- **Exemptions.** Only those two cases: no surface delta, or a removal-only one. There is
  no opt-out marker, because an opt-out would get used every time. A one-word description
  fix owes a one-row ledger, `| "<the sentence>" | wording only, no claim changed | — |`,
  which takes a minute.
- **Errors carry the fix**, as the gate's other checks do:

```
the PR changes tool:ipython, which the model reads; the body owes `## Claims ledger`
  with a table (claim · check · result) — `just pr-body` prints the skeleton
```

**`pr_body.py`** imports the same `surface_delta(base)` and, when it is non-empty, prints
the owed sections as skeletons. Each is a header row and a separator with the columns in a
comment, and no data row, so an unfilled skeleton fails the gate on purpose.
`surface_delta` lives in pr_body.py beside `base_commit` and `diff_stats`, and
check_pr_metadata.py imports it the way it already imports `tally`, so there is no second
copy. pr_body.py also starts printing `## Seen red` (§0 item 4).

**`forge_pr.py compose_body`** skips a counted section whose heading the author's prose
already has. That is five lines, and it ends the open-then-edit workaround of §0 item 6.
It also matters more now: if a body edit does not re-run the `title` job (§13, question
6), the body the PR is opened with is the body the gate reads.

**Selfcheck.** Each case is the check disabled in turn, and the selfcheck must fail for
that check's own reason (040):

| case | expected |
|---|---|
| no delta, empty body | `[]`, and the forge is not asked |
| lock absent at base, keys at HEAD | `[]` (seeding) |
| removal only | `[]` |
| changed, no ledger heading | one error naming `## Claims ledger` and the changed key |
| changed, heading holds only the template comment | error (comments are stripped) |
| changed, heading holds prose, no table | error |
| changed, header and separator, no data row | error |
| changed, `### Claims ledger` | error (wrong level) |
| changed, one data row | `[]` |
| added, ledger only | two errors: `## Neighbour matrix`, `## Dogfood` |
| added, all three with rows | `[]` |
| a table under the next section, not this one | error (the section ends at the next `## `) |

## 7. The tier-2 journeys, starting with documents

**One cassette per tool family.** `fixtures/journeys/` is not a documents directory.
Documents come first only because D171 is the first change to have run the method. Each
later full-method change adds `<family>-questions.json` from its own play session (step
16), built on fixtures of its own and scored the same way. Two examples:
- a `bash` family: a reducer's summary checked against the unreduced output of the same
  command;
- an `edit` family: the file's bytes after a scripted edit sequence.

The rest of this section is the first cassette.

**Where it lives.** It is a second test function in `crates/runtime/tests/behavior.rs`,
marked `#[ignore = "tier-2 journey: \`just journeys\`"]`, which replays every cassette
under `crates/runtime/tests/fixtures/journeys/`. `check_behavior.py` runs
`--test behavior` without `--ignored`, so the T1 baseline never runs it, and `just
journeys` (and so `just postmerge`) always does. That makes 15 tier-2 journeys, up from
14. It reuses `drive` and `evaluate` rather than copying them.

**Binary files.** A cassette's `workspace` map stays text. A new `workspaceFiles` map copies
repo fixtures by relative path: `{"layout.pdf": "documents/layout.pdf"}`, resolved under
`crates/runtime/tests/fixtures/`. Paths go through the same `is_relative` check `workspace`
uses. Base64 in the JSON was rejected: it doubles the bytes, a reviewer cannot read it, and
it forks a fixture from the file every other test uses.

**The kernel.**
- A `"session": "documents"` cassette gets `builtin_tools_with(false, Some(Documents {
  home: <scratch>, ..yi_runtime::documents(&home) }))` plus `ipython_tool` over a real
  `KernelService`, as `kernel_lane.rs` boots one in its own journey.
- Cassettes without the key are unchanged: `builtin_tools()`, or their `stubs` when they
  carry any (`cassette_tools`, `behavior.rs:263-278`).
- It needs the kernel venv, which the postmerge image already builds for the kernel
  journeys: its `uv` is there, and "the image carries just and the uv the kernel journey
  boots", `postmerge.yml`.
- `KernelService` takes the real HOME, as `kernel_lane.rs` does, because a scratch HOME
  would build a second venv. Copies stay in the scratch home.
- Unlike a T1 cassette, a failed assertion **fails the test**. A journey's red is a
  regression, not a baseline verdict. The twice-replayed determinism check is skipped,
  since a second kernel boot buys nothing the assertions do not already check.

**Scoring.** The faux model's final text is scripted, so it proves nothing. The journey
scores the **tool results**, which is where the D171 defects were, using three new
assertion kinds that T1 cassettes can use as well:

- `toolResultLine {callId, needles: [...]}`: one line of the result holds every needle.
  This is the row rule of 085 ("a frame assertion reads the row"). A misattributed table
  still contains `CPSC 380` and `5:30PM` somewhere in the output, but not on one line.
- `toolResultLacks {callId, needle}`, for raw markup (`\rtf1`) and dead advice (`rg -a`).
- `toolResultBytesAtMost {callId, max}`, for a first-look ceiling.

**The questions** are built only from the repo's own fixtures, and every answer is traced
in the cassette's `provenance` to a source that is not Yi. The fixtures were printed by
macOS Quartz (`pdfinfo` → `Producer: macOS Version 26.7 … Quartz PDFContext`) from authored
text, and that truth was checked for this document by a second engine: `pdftotext -layout`
and pandas.

| # | turn (scripted calls) | assertion | truth, checked 2026-09-10 | D171 class |
|---|---|---|---|---|
| 1 | `read layout.pdf find="Operating Systems"` | line holds `CPSC 380`, `Mon Wed 5:30PM`, `Keck Center 156` | poppler: `CPSC 380 Operating Systems  Mon Wed 5:30PM to 6:45PM  Keck Center 156` | 1 |
| 2 | `read single.pdf` | line holds `(5, -10)` | poppler: `1. (5, -10)` | 1 |
| 3 | `read roster.xlsx`; ipython `pandas.read_excel("roster.xlsx", header=None)` | both results have a line holding `Łukasz` and `B+` | pandas: `Łukasz`, `Enrolled 3.00`, `B+` in one row, after two empty rows | 1 (two engines), 3 |
| 4 | `read ledger.xlsx limit=3`; then what its hint says: ipython `print(int(pandas.read_excel("ledger.xlsx")["Amount"].sum()))` | first result contains `pandas.read_excel in ipython`; second contains `1803000` | pandas: `1803000` | 6 |
| 5 | `grep "Espresso"`; `grep "fonttbl"` | first lists `brief.docx` and `brief.rtf`, lacks `rg -a`; second contains `No matches found` | anydoc on both: `\| Espresso machine \| Zoë \| ordered \|` | 7, 2 |

Question 4 is the one no T0 test replaces. `a_large_sheet_points_at_a_pandas_route_that_
works` runs the converter's interpreter directly; the journey runs the hint through the
`ipython` tool of a live session, which is the route the hint actually names. If the
session kernel and the converter ever resolve different interpreters, only the journey goes
red. **Seen red, at implementation:**

- drop `openpyxl` from the extras, with `--venv` in a scratch dir → question 4 red;
- send PDF table pages back through anydoc's Markdown → question 1 red;
- turn off grep's document search → question 5 red.

The cassette is drafted in Appendix C. A byte ceiling over the whole path was rejected:
`a_long_document_opens_with_its_outline_and_a_short_first_look` already pins the first
look, and a sum over a scripted path defends nothing that test does not.

## 8. Testing doctrine additions

Four bullets go into `.ruler/080-testing.md`, directly after the "A fixture is the
production shape" bullet. Their exact text is Appendix B. What each one extends:

- **B1, a hint is a claim and its test runs the remedy.** It extends "External ground
  truth over self-confirmation": the remedy is run, not string-matched. It is also an
  application of "Never source-grep": the proof is running the remedy, not asserting on
  the hint's wording.
- **B2, one bad part, and a limit from both sides.** It extends "A fixture is the
  production shape" to inputs that have parts, and the loud-caps rule (045, "the test for
  a cap is the test that trips it") to the value just under the limit.
- **B3, a second reference for any transform that can be confidently wrong.** It extends
  "External ground truth" and the row rule of 085.
- **B4, state has a race test and a stale test.** It extends "seen red": a cache that has
  never met a second thread or an unchanged timestamp has only been seen green.

A fifth bullet was drafted, "never a user's file or a benchmark's answer key", and folded
into B3's provenance sentence instead of standing alone. None of the four is a gate. You
cannot detect "this module keeps a cache" without heuristics over source text, which is
the source-grep the doctrine forbids. The rules bind through `## Seen red`, which already
names each test's failure and its fixture's provenance, and through the reviewer's reading.

## 9. Mandate scope

| change | what it owes |
|---|---|
| **new tool**, **new kernel extra**, or **new input kind** for an existing tool | the full method, A to F. `## Claims ledger`, `## Neighbour matrix` and `## Dogfood` (census buckets and the replay's before/after table). A journey question per P1 found in play |
| **surface change**: a description, schema, parameter doc or identity edit | steps 1 and 2 for the changed sentences, with every changed hint executed once. `## Claims ledger` |
| anything else | today's rules: `## Seen red`, fixtures, done-bar |

The gate sees the first two rows only as lock keys: an added key is row one, a changed key
row two. A **new input kind that changes no description** is invisible to it (a wheel
update inside its pin that brings a new format, say, or grep learning a kind without its
description saying so). For that the obligation rests on the 080 text and on review, and
the skill says so. It is a known limit of a deterministic trigger, accepted rather than
closed with a heuristic.

**The before/after table** goes in `## Dogfood`. Quoted sentences go in `## User outcomes`,
as 080 already requires for a sentence a test pins. No new section is added for the
narrative: the Summary tells it.

## 10. Refute pass on this proposal

Every gate and test is listed with how it is proven red for its own reason. What could not
be proven that way was cut.

| item | proven red by | kept |
|---|---|---|
| surface lock | change one word of ipython's description without `--update` → guardrails lane fails naming `tool:ipython`; revert the widened table → the lock reports `tool:ipython` and `tool:ask_user` removed | yes |
| widened budget table | the same revert moves `tools` down by the measured bytes, which the `--update` diff shows | yes |
| PR section gate | the twelve selfcheck cases of §6, each check disabled in turn | yes |
| `compose_body` dedupe | a selfcheck case: prose carrying `## Summary` plus pr_body output yields one `## Summary` | yes |
| journey questions 1, 4, 5 | the three mutations of §7 | yes |
| journey question 3 | make the empty-row filter stop at the first empty row → Łukasz's row, which sits after two empty rows, is gone from `read`'s result while pandas still shows it | yes |
| journey question 2 | question 1's mutation, if `single.pdf`'s page is detected as a table: `(5,` and `-10)` land in separate cells. Confirm at implementation, or cut the question | provisional |
| the player's table is the CLI's | by construction: both call `session_tools` and `attach_runtime`, and the lock pins the names; phase 2's play session calls every name in `tools.md` once | yes |
| harness keeps compiling | rename a `pub fn` the example calls → lint lane red (`clippy --all-targets`) | yes |
| `dogfood.py --selfcheck` | sampler: a tree of known sizes keeps the smallest and largest per extension; `<run>` guard: `/x` and `a/../b` refused; counter: two calls get two numbers | yes |
| 080 rules B1–B4 | not gates; bound by `## Seen red` | yes, as rules |
| "Neighbour matrix on every tool change" | cannot tell a neighbour-relevant change from any other deterministically | **cut**; required only for added keys |
| "read the replay transcript as the model would" | not mechanical | **cut** as a mandate; kept as a skill step |
| a latency budget per input class in T0 | wall clock on a shared runner flakes; 010's only wall-clock gate runs outside CI for that reason | **cut**; replaced by the negative path observed not to spawn |
| a detector for "module keeps a cache" | heuristics over source text | **cut** |
| a journey byte ceiling | duplicates the first-look test | **cut** |

## 11. Cost

| item | where | lines (estimate) |
|---|---|---|
| `session_tools` moved out of `main.rs`; `AgentSession::tools()` getter | `crates/runtime/src/{lib.rs,session.rs}`, `crates/cli/src/main.rs` | +12 src, −6 src |
| widened tool table, stub bridge, `TOOL_SURFACE` lines | `crates/runtime/tests/request_budget.rs` | +45 test |
| lock read, `--update`, selfcheck | `scripts/guardrails/check_request_budget.py` | +45 py |
| `surface_problems`, section parser, selfcheck | `scripts/guardrails/check_pr_metadata.py` | +70 py |
| `surface_delta`, skeletons, `## Seen red` | `scripts/pr_body.py` | +35 py |
| call into the gate, `compose_body` dedupe, the lock in `cmd_ratchet` | `scripts/forge_pr.py` | +15 py |
| `workspaceFiles`, `session: documents`, three assertion kinds, the journey function | `crates/runtime/tests/behavior.rs` | +110 test |
| the cassette | `crates/runtime/tests/fixtures/journeys/documents-questions.json` | ~160 JSON |
| census, play (session wiring, broker, `--ask`), venv modes | `crates/runtime/examples/dogfood.rs` | ~240, priced as test LOC |
| sample, mine, call, stop, replay, report, clean, selfcheck | `scripts/dogfood.py` | ~260 py |
| recipe, glob, selfcheck line | `justfile`, `check_test_size.py`, `check_guardrails.sh` | +4 |
| rules, skill | `.ruler/080-testing.md`, `.ruler/skills/yi-dogfood/SKILL.md` | +20, ~70 |

- **src LOC:** about +6 net, in phase 1, well inside the +150 free band, so no growth memo
  is owed.
- **Test LOC:** about +395, plus the existing example once the glob widens, each through
  `just ratchet`.
- **Request budget:** the `tools` and `total` baselines rise by what sessions already pay,
  at least 1.9 KB counted from the source strings, with the exact figure being the
  `--update`'s. Zero bytes are added to any request.
- **CI:** the lock rides the `request_budget` test run the guardrails lane already makes.
  The section check is a pure function in the one-second `title` job. The journey runs
  postmerge only. Measured here with a warm venv, the kernel journey `kernel_lane` takes
  1.37 s and 27 document tests take 1.33 s. The new journey's five turns and one kernel
  boot are estimated at 2 to 5 s on this machine; the runner's figure is unmeasured (§13).
- **Dogfood time per change, estimated:** D171 ran all three rounds and fixed 43 of 47
  findings on one day (2026-09-10, per its §12). For a new tool or kind: census 30 to 60
  minutes including reading outliers, review 1 to 2 hours, play 1 to 2 hours (38 calls in
  D171), replay 15 minutes. For a description change: minutes, for the ledger rows and one
  run of each changed hint.
- **Money: none.** No gate or journey calls a model.

## 12. Rejected, with reasons

- **Live model runs on every change.** They are paid and nondeterministic, and money in a
  gate is what plan law 3 forbids (080 T3). The fresh-agent eval stays opt-in, user-run and
  ledgered in `docs/eval-ledger.md`.
- **An LLM judging the ledger or the dogfood table.** The rule is deterministic over LLM
  loops: no LLM-judged firing. The gate checks presence and shape, and a person reads the
  cells.
- **A path-list trigger** (§5). It fires on bug fixes and misses a description built from
  a constant or a wheel.
- **Checking in the census corpus or play inputs.** They are personal files and benchmark
  answer keys. The journey uses the repo's fixtures; the census runs on copies deleted
  after.
- **Environment-gated test files, and a `yi dogfood` subcommand** (§4.1).
- **Base64 fixtures in cassettes** (§7).
- **A loop-level player now** (§4.5). It waits for its trigger.

## 13. Open questions

1. **Should `prompt:doctrine` and `prompt:mode` be lock keys?** That would give a broader
   trigger and more ledger rows. The proposal leaves them out.
2. **Should a document task be added to `evals/fixtures/live/`?** That is the folder
   `evals/run.py --live` reads (`run.py:36`, `:323`); `evals/fixtures/tasks` is the faux
   `--dry` suite. The advisory `live` job in `pr.yml` would then run it on every PR once
   `OPENROUTER_API_KEY` is set, under its `--cap-usd 1`. That is paid and can only be priced by a paid run, which this session did
   not make, so the proposal does not include it.
3. **Pre-registration (step 3):** mandatory in `## Dogfood` (a truth column in the replay
   table), or left as a skill step? The proposal leaves it as a step; the table has room
   for it.
4. **Milestone.** The issue sits under "Release and repository plumbing". "Forge plumbing"
   would also fit.
5. **The journey's runner time**, which only a postmerge run on the forge can measure.
6. **Does a body edit re-run the `title` job?** One observation, on this plan's own PR
   (#377). It was opened with `--refs` and no body, and at about 20:05Z the body was
   replaced with `forge_pr.py pr edit`. After the gate finished, `/actions/tasks` still
   showed a single `title` run (8127) for the head. There is no control run, so this is not
   yet settled. If an edit does not re-run the job, the gate reads the
   body the PR was opened with, and a fixed body needs `just pr rerun`. §6's `compose_body`
   fix makes opening with the full body the normal path either way.
7. **Can `attach_runtime` be driven from an example or a T0 test as it stands?**
   `build_session` also claims a lane and attaches a store, and the player needs neither.
   Verify this in phase 1. If it cannot, the lock falls back to the hand-built table (§5)
   and the player adds `plan` and `todo` by hand, over `session.store_handle()`.

## 14. Phases

All three phases are T0, T1 and T2 changes. The only src change is phase 1's move of
`session_tools` and the `AgentSession::tools()` getter, about +6 net lines. They are one
issue, #376, in three PRs, merged in order.

**Phase 1: the mandate** (`Refs #376`). What lands:
- `yi_runtime::session_tools`, called by `main.rs` in place of its closure, and
  `AgentSession::tools()`;
- the widened `request_budget.rs` table and the `TOOL_SURFACE` lines;
- `check_request_budget.py` with the lock, `--selfcheck`, and a line in
  `check_guardrails.sh`;
- `tool_surface.json`, seeded in the same commit as its reader;
- `surface_problems` in `check_pr_metadata.py` and its selfcheck;
- `surface_delta`, the skeletons and `## Seen red` in `pr_body.py`;
- the `compose_body` dedupe;
- the 080 bullets (Appendix B) and the skill (Appendix A), with
  `npx @intellectronica/ruler apply`.

Commit order, which today's verbs cannot produce on their own:

1. `Ratchet: request budget tools X -> Y`: `check_request_budget.py --update`, run by hand,
   committed alone. `forge_pr.py cmd_ratchet` does not run that script (`forge_pr.py:187`),
   and it writes `Ratchet: request budget` with no `X -> Y` (`:205-206`).
2. The code commit, carrying `tool_surface.json` beside its reader. `check_commit_style.py`
   allows a new baseline beside the code that reads it (`:151-154`), but `cmd_commit` moves
   any dirty baseline into a Ratchet commit first (`forge_pr.py:219-221`), so this commit is
   made with `git commit` and explicit paths. It carries the D-row, the version bump, the
   changelog row and the ADR (§15).

Phase 1 also adds `check_request_budget.py --update` to `cmd_ratchet`'s list, with its
`X -> Y` subject (+5 lines), so every later lock change goes through `just ratchet`.

**Phase 2: the harness** (`Refs #376`). What lands:
- `crates/runtime/examples/dogfood.rs` and `scripts/dogfood.py` with `--selfcheck` (and its
  line in `check_guardrails.sh`);
- the `dogfood` recipe;
- the `check_test_size.py` glob, with its `Ratchet:` commit first.

Proof: a census over `crates/runtime/tests/fixtures/documents/` reproduces the outcome
documents.rs asserts for each fixture: `scanned.pdf` refused, `mixed.pdf` converted with its
image page named, the other nine converted. A play session that calls every registered tool
at least once, `plan`, `todo` and `ask_user` included, is then replayed and gives an
identical table.

**Phase 3: the journey** (`Closes #376`). What lands:
- the `behavior.rs` extensions and the cassette;
- `## Seen red` carrying the three mutations of §7.

## 15. Records this change owes

- **D-row**: the next free number. That is D173 today, but only if #372 (D172), #374
  (D169) and #365 (D170) still hold theirs. Re-read `docs/ARCHITECTURE.md`'s decision log
  and every open PR's diff immediately before writing it (090). The draft:
  - decision: "a change to the tool surface a session registers is locked and owes a
    claims ledger; an addition also owes a neighbour matrix and a dogfood table; the method
    is the yi-dogfood skill and `just dogfood`; D171's play path is a tier-2 journey";
  - why: "D171's worst defects were claims that unit tests and fixtures passed, found only
    by real files and a played session";
  - reversible via: "delete the lock and `surface_problems`; the skill and harness stand
    alone".
- **Version**: `docs/ARCHITECTURE.md` → 0.210.0 unless main or an open PR has moved past it
  (0.208.0 is #372's, 0.209.0 is #374's), in phase 1's commit, with its
  `docs/CHANGELOG.md` row citing the issue.
- **ADR**: `python3 scripts/adr.py <N>` renders `docs/solutions/adr/d<N>.md` and its index
  line from the row.
- **Feature ledger**: no row. Nothing user-visible changes; the gate and the journey are
  process.
- **Growth memo**: none. The net src delta is about +6, inside the free band.

---

## Appendix A: `.ruler/skills/yi-dogfood/SKILL.md`

```markdown
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
```

## Appendix B: `.ruler/080-testing.md` additions (exact)

These go directly after the bullet that begins "A fixture is the production shape":

```markdown
- A hint that names a remedy is a claim, and its test runs the remedy where the model would:
  "pandas reads this" is proven by pandas reading the fixture in the kernel venv, never by the
  sentence being present. The spreadsheet hint shipped naming a route the venv could not take
  (pandas without openpyxl); only running it found that.
- An input made of parts carries a fixture with exactly one bad part, and every limit is tested
  at the limit and at the limit plus one. An eleven-page PDF with one image-only page was
  refused whole; the census found it, not a fixture.
- A transform that can be confidently wrong — a table, a layout, a decode — is checked against
  a reference that did not come from it: the same content in a second format, or a second
  engine's reading, asserted a row at a time. A schedule came back with a neighbour's slot
  pinned to the right course name, which a check for the course name alone passes. The fixture
  comes from a real producer, named in `Seen red` — never a user's file or a benchmark's
  answer key.
- Anything that keeps state across calls — a cache, a staging file, a key — has a test that
  runs N threads on one new input and one that changes the content while keeping the
  timestamp. Forty of sixty parallel first reads of one document failed on a shared staging
  name that every test run one call at a time had passed.
```

## Appendix C: the journey cassette (draft)

`crates/runtime/tests/fixtures/journeys/documents-questions.json`, with one of its five turns
shown whole. The others follow the table in §7:

```json
{
  "id": "documents-questions",
  "description": "questions with checkable answers over the repo's own document fixtures, asked through a live session with the ipython kernel; each assertion reads one line of a tool result, because D171's wrong answers kept every substring and lost the row",
  "provenance": {
    "kind": "authored",
    "truth": "the text each fixture was printed from (macOS Quartz); layout.pdf and single.pdf cross-checked with poppler's pdftotext -layout, the sheets with pandas, brief.docx against brief.rtf"
  },
  "session": "documents",
  "workspaceFiles": {
    "layout.pdf": "documents/layout.pdf",
    "single.pdf": "documents/single.pdf",
    "roster.xlsx": "documents/roster.xlsx",
    "ledger.xlsx": "documents/ledger.xlsx",
    "brief.docx": "documents/brief.docx",
    "brief.rtf": "documents/brief.rtf"
  },
  "turns": [
    {
      "user": "When and where does CPSC 380 meet?",
      "responses": [
        {"toolCall": {"id": "schedule", "name": "read", "arguments": {"path": "layout.pdf", "find": "Operating Systems"}}},
        {"text": "answered from the schedule"}
      ]
    }
  ],
  "assertions": [
    {"kind": "toolResultIsError", "callId": "schedule", "value": false},
    {"kind": "toolResultLine", "callId": "schedule", "needles": ["CPSC 380", "Mon Wed 5:30PM", "Keck Center 156"]},
    {"kind": "toolResultLine", "callId": "answers", "needles": ["(5, -10)"]},
    {"kind": "toolResultContains", "callId": "ledger-head", "needle": "pandas.read_excel in ipython"},
    {"kind": "toolResultContains", "callId": "ledger-sum", "needle": "1803000"},
    {"kind": "toolResultLacks", "callId": "grep-phrase", "needle": "rg -a"}
  ]
}
```
