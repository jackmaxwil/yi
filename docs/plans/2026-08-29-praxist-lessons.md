# Praxist — what a research orchestrator knows that Yi does not

```
status:  EVIDENCE 2026-08-29. Not a work plan: the work is nine rows in
         docs/TODOS.md (`C9` `I2` `J9` `M6` `M7` `N10` `O8` `O9` `P7`), worked in
         that file's order like every other row. This document is why they exist,
         what was measured to justify them, and what was rejected — the negative
         space is most of the value. `M6`, `N10` and `P7` each need a D-row before
         code; ARCHITECTURE.md held D73 at version 0.66.0 when this was written and
         another session in this shared tree may have claimed the next number since.
date:    2026-08-29
sources: sapientinc/PRAXIST @ shallow clone 2026-08-29 (ref/tools/PRAXIST) —
         AGENTS.md §§9-11/18/29, docs/concepts/config_discipline.md,
         docs/guides/cost-optimization.md,
         docs/guides/peer-local-structured-memory-long-context.md,
         praxist/core/{prompt_layout,protocol,redaction,ledgers,execution_guards,
         runtime_guard_policy}.py,
         praxist/plugins/agent_runtimes/claude_sdk/{delete_guard,liveness,adapter}.py,
         tests/hardening/{_env_read_audit.py,env_read_allowlist.txt},
         scripts/dev/{leakage_audit.sh,run_guardrails.py}
         · Yi: crates/runtime/tests/request_budget.rs, crates/runtime/src/{wall.rs,
         ext/assemble.rs,ext/install.rs,lib.rs}, crates/permission/src/{decide.rs,
         safety.rs}, crates/tools/src/{reduce.rs,sandbox.rs,builtins.rs,tool.rs},
         crates/context/src/{budget.rs,convert.rs}, crates/ai/src/{openai.rs,
         openai_responses.rs,request.rs}, crates/types/src/message.rs,
         scripts/guardrails/{check_guardrails.sh,check_env_surface.py,
         check_request_budget.py,check_blob_size.py}, docs/{FORGEJO.md,TODOS.md}
note:    PRAXIST is not in YI_DESIGN.md Appendix A and is not a port source. It is
         study material. Every row above is written as "build fresh against the
         contract", never "port"; taking code would need an A-span entry first.
```

Praxist is an autonomous research orchestrator — N parallel agent peers over
generations against a measurable objective, with evidence lanes, synthesis, and
lifecycle control. Wrong product, right problems. It drives Claude and the reference
agents in a loop with budgets, guards, replay, and prompt caching at a scale Yi
has not reached, and it has the scars: 125k lines of source, 137k lines of
tests, and a 6,667-line file that is a cautionary tale rather than a source.

## 1. Thesis

The mechanisms worth taking share one shape. **A value that is unknown is being
recorded as a value that is known.**

- A prompt prefix stable within a process but not across processes reads as cached and is not (`J9`).
- A grep result capped at 200 hits reads as complete and is not (`C9`).
- A provider that reports no usage reads as a free turn and was not (`M6`).
- A wall denial naming a path but no rule reads as policy and is prose (`N10`).
- A stream emitting keepalives reads as progress and is a hang (`M7`).

Praxist states the answer as a product principle rather than a runtime detail
(AGENTS.md §11):

```text
capture first, label uncertainty, continue when safe
```

with the corollary that **observability never gets to kill a result**. Their
budget-accounting failure degrades to a warning so completed findings survive;
their stall watchdog at 300 seconds logs and does nothing else, because *"the
isolated session remains bounded by its existing runtime and generation
deadlines."* Detection and termination are separate authorities. Every row above
that adds a detector adds it as a label: `M7` cannot end a turn, `N10` downgrades
a refusal to a warning rather than adding a new veto.

Second principle, from their env-audit docstring — *"Anything not detectable
here doesn't count."* A gate states its own ceiling. Yi already does this once,
at crates/runtime/src/wall.rs:50. `J9`, `O8` and `O9` each owe the same sentence.

## 2. What survived contact with the code

Nine observations came out of the Praxist read. Checking them against Yi changed
four and killed two. This table is the reason the rows are as small as they are.

| observation | what Yi actually does | outcome |
|---|---|---|
| Yi's prefix gate measures bytes, blind to a cwd or clock in the prefix | `J4` already asserts system and tools byte-identical across two turns, no message re-rendered (D51), and block 0 stable across a yard change and a mid-session attach | **Narrowed.** All three render in one process, so an ambient value embeds identically in both and passes. Block 0 is Identity + Doctrine, two `include_str!` constants — frozen by content, not by a gate. `J9` is the residue only |
| Yi has no full-result-on-disk mechanism | `reduce()` tees oversized output, names the path in the text, and honours T19: lossy with nowhere to recover from is worse than unreduced. `full_output_path` exists on the Pi-compatible `BashExecution` type and nothing sets it | **Narrowed and cheapened.** Mechanism, rule, and wire slot all exist. `C9` extends them to grep and glob and wires the field. No new `ToolOutput` field — the path travels in the text, which is the channel that reaches the model |
| Yi's `Usage` has no unknown state | Confirmed: `unwrap_or(0)` on every field in both adapters, and `Usage::zero()` is also the faux provider's deliberate value | **Stands** — `M6`. Free, unknown, and faux are one byte pattern |
| `Wall::check` returns unattributable prose | Confirmed (wall.rs:53). The advisor two directories over already has `AdvisorySeverity::{Note,Warn,Hold}` plus `AdvisoryOutcome`, which Praxist has no equivalent of | **Stands, and shrinks** — `N10` reuses the advisor's vocabulary and delivery path instead of adding a `WallDecision` to yi-types. Yi was ahead on the advisor and behind on the wall, using two vocabularies for one concept |
| A `sitecustomize` guard would close the hole wall.rs:50 admits | `P1` already records the hole **and its cause**: the kernel is long-lived and started outside the sandbox | **Redirected** — `P7`. The recorded cause names the fix. Praxist runs a Python guard layer *and* a command validator and still loses to `%%bash`, `subprocess`, `ctypes`, and an alias captured before import |
| Yi's permission ladder cannot say whether a restriction is a prompt or a kernel | It can. `Decision::Contain` is the OS-sandbox rung, `workspace_sandbox()` returns `None` where the platform has none, and lib.rs:46 states the policy: a contained decision degrades to a question rather than to an unenforced allowance. Seatbelt denies egress by default | **Mostly dies.** Yi implements the fail-safe half, and more cheaply than Praxist's capability manifests. What remains is `O1`'s existing Linux gap and `P7` |
| Env is read ad hoc across Yi | Confirmed: 29 `env::var` sites across 9 crates; five are outside any ingress boundary | **Stands** — `O9`, with a crate-scoped ban rather than Praxist's per-site allowlist |
| Yi's compaction emits prose where a typed card would survive | Confirmed, but see §3 | **Deferred.** `E2` already owns the space |
| Yi's liveness is one clock | Worse: the only clock is `timeout_read(60s)` on the ureq transport, which measures the gap between bytes on the wire, not progress in the turn | **Stands, sharpened** — `M7`, one clock on content deltas |
| Add a test asserting the local runner and CI run the same commands | docs/FORGEJO.md: *"Yi's gate is the justfile... A workflow here is a thin caller — never a second copy of the rules, which is the failure `.github/` was deleted to avoid (0.59.0)."* And `check_guardrails.sh` already counts sub-gate failures in `run()` and exits 1 | **Dies.** Yi solved it structurally and better. The residual risk CLAUDE.md records — `cargo test \| grep` reporting grep's exit — lives at the invocation site, where no gate reaches |

## 3. Rejected, with reasons

**Praxist's `delete_guard.py`, 6,667 lines.** The largest file in that repository
is a string-matching sandbox. It parses `rm`, `find -delete`, `mv`, `rsync
--delete`, `sed -i`, `tar`, `zip`, `shred`, awk redirections, `git clean`, make
and ninja include targets, nested `sh -c` to depth three, `python -c` bodies,
`ctypes.CDLL(None)`, `trap DEBUG`, and `LD_PRELOAD`. It still cannot win:
misconfiguration fails open (`if not allowed_roots: return _allow()`), tampering
with its own guard environment is a *warning* rather than a deny, and depth-four
nesting is denied rather than parsed — an admission the grammar ran out.

Yi's 93-line `wall.rs` with an honest ceiling comment is the better artifact. It
says what it is. The real upgrade is OS enforcement, which Yi already has on one
platform and which Praxist's own `protocol.py` has a constant for
(`SANDBOX_ENFORCEMENT_OS_SANDBOX`) without reaching for it. `N10` takes the
structure — attributable rules, a warn tier — and `P7` takes the enforcement.
Neither takes the arms race.

**A second guard layer inside Python.** Proposed, then rejected against `P1`. It
is the same arms race one layer down; being honest about which escapes beat it
is still losing to them. `P7` instead.

**A typed continuity card in compaction** — `open_questions`, `dead_ends`,
carried beside the prose summary and re-emitted verbatim rather than
re-summarized. Praxist's peer memory does this and it is a good idea there,
where a peer spans many sessions across generations. For Yi it is ~150 lines
plus a schema change for a failure — load-bearing facts lost on a *second*
compaction — that has never been observed here. `E2` already owns the adjacent
job and is the cheaper first move: check whether the summary dropped something
before building a structure to stop it. Build the card when a loss is seen.

**A vocabulary-leak gate**, after Praxist's `leakage_audit.sh`, proving reference
vocabulary does not cross into generic crates. Drafted and cut: the escape hatch
would be used by most legitimate matches — this codebase deliberately cites its
sources (`Pi \`convertToLlm\``, `the reference's exact recovery marker`) — and the failure
it would prevent, drifting into upstream parity, is a judgment failure a grep
does not reach.

**A `read_tool_result(ref, offset, max_chars)` tool.** Praxist needs it because
its agent cannot open the artifact. Yi's `read` takes an offset and the recovery
file is in the workspace. A second reader would be a tool for a capability that
already exists.

**Their plugin and manifest system.** `plugin.yaml` carrying `stability`,
`protocol_version` and a `capabilities` list lets core fail fast on an
unsupported request. Yi has thirteen fixed crates and an allowlist; a registry
buys nothing and costs a discovery path.

**Coverage as a gate.** 90% branch and 95% statement, producing 137k lines of
tests against 125k of source, a 5,700-line `frontier.py` and a 5,242-line
`gems.py`. Their own testing doctrine asks for long-lived contract tests over
tests asserting incidental shape — a percentage cannot tell the two apart, and
will always be satisfied more cheaply by the second kind. Yi's shrink-only
ratchets cap the mess without rewarding volume.

## 4. Two principles worth keeping verbatim

**Result preservation** (AGENTS.md §11), quoted in §1, with its corollaries:
*"Weak provenance is acceptable when clearly labeled"* and *"Replay, audit,
attribution, and budgets should explain uncertainty rather than drop results."*
The hard line: do not harden in a way that stops an agent finishing promising
work unless there is a concrete integrity, secret, safety, or irrecoverable-state
risk. Yi's advisor `Hold` and `Wall::check` both have the power to stop a turn;
`N10` is the first pass at asking whether the weakest of them earns it.

**Metrics lie in pairs**, from their cost guide: *"A high cache hit rate can
coexist with excessive logical token use when every new thread repeats broad
bootstrap reads."* Yi's request-prefix ratchet is exactly the metric this warns
about — it can shrink while total spend rises. `J5` already records the honest
version of this limit (request bytes, not billed tokens; `J3` owns the rest).

## 5. One observation about gates, kept because it applies to ours

Praxist's `test_public_docstrings_are_stable_contract_text` bans patch-note
prose. Its scope is docstrings of public top-level items. So
`praxist/plugins/workflow_stages/research_loop/backend/agent.py:212` reads
`# v2026-05-04 R2#2 fix: orchestrator-initiated drain via STOP_SIGNAL`, and line
1794 reads `# R2#8 fix:`. The rule is written repo-wide, the enforcement is not,
and the private dialect grew in the gap.

Yi's `check_comments.py` is stronger for the opposite reason: its closed
vocabulary rejects any capitalized `Word:` prefix other than `Incident:` and
`Invariant:`, so it fails on the unanticipated case rather than allowlisting a
location. `J9`, `O8` and `O9` should each be read against that standard before
they land.
