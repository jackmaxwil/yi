# evals/ — task-eval adapters

Pure Python, standard library only. No Cargo.toml, no workspace member, no
dependency of its own: harbor and pier are the host harness's installs and the
adapters import them; everything the self-test exercises is stdlib.

```
adapters/yi_usage.py       run command + usage parse + fingerprint (shared, pure)
adapters/yi_harbor/agent.py  harbor BaseInstalledAgent subclass
adapters/yi_pier/agent.py    pier BaseInstalledAgent subclass (+ install spec, allowlist, extras)
selftest.py                dry run: no docker, no API, no keys, no harness
fixtures/                  a recorded faux transcript and a v4 session file
```

Contracts: YI_DESIGN §15.2 (gates E1–E9), §15.4 (AA column → Yi source),
§15.5 (adapter shape), Appendix A.12 (the exact harness spans).

## Self-test

```
python3 evals/selftest.py
```

Exit code is the gate. It pins the run command's E6/E7/E8 contract, the
camelCase usage parse (E5), the all-or-none token rule (E9), corrupt-line
tolerance, and pier's three extra columns.

## Installing the binary

Both adapters install one static `x86_64-unknown-linux-musl` binary at
`/usr/local/bin/yi` — one step, so pier's 360 s agent-setup cap and harbor's
install phase both stay cheap (§15.3 lever 8).

- `EVAL_YI_BINARY_URL` — release URL to `curl`. The eventual default; no
  release lane serves one yet.
- `EVAL_YI_BINARY` — local musl build, uploaded into the container. harbor
  only; pier's install spec is lowered into a Dockerfile and needs the URL.

Both paths verify with `yi --version` before the trial starts.

## Running a suite

```
export ANTHROPIC_API_KEY=...            # passed through under its own name
export EVAL_YI_BINARY=target/x86_64-unknown-linux-musl/dist/yi
PYTHONPATH=evals/adapters harbor run \
  --agent yi_harbor.agent:Yi \
  -d terminal-bench/terminal-bench-2-1 \
  --model anthropic/claude-opus-4-5 --n-attempts 3
```

`-d scale-ai/swe-atlas-qna` is the same command with another dataset. pier is
the same shape with `pier run --agent yi_pier.agent:Yi`.

SWE-Atlas QnA is graded **only** from `/logs/agent/answer.txt` inside
`<<FINAL_ANSWER>>` tags, and the verifier runs even after an agent timeout
(E4) — so drafting the answer file early and refining it in place is worth
real points. That is task and prompt discipline: the adapter never injects
instruction text of its own.

## Budget discipline

Real-model rollouts are budgeted, deliberate, and ledgered (plan law 3).

- **Timeouts are never retried.** A failed run is a result, not a do-over —
  pier's retry-exclude defaults already encode this, and the adapters do
  nothing clever about it.
- **Every real run appends a row to `docs/eval-ledger.md` before any claim
  cites it** — run-id, suite@rev, config fingerprint, §4 metrics. Claims cite
  ledger rows or they are vibes.
- **Defaults are what is benchmarked.** A high-effort variant is a separate
  ledgered row, never a quiet edit to the scored config.
- μ metrics for a run come from the session-mining extractor over the trial's
  collected session directory. One extractor owns that schema; the adapters
  emit harness fields only.

## Fixtures

`fixtures/ask_events.jsonl` is a real `yi ask --json` transcript recorded
offline against the faux provider:

```
./target/debug/yi ask --model faux/faux-1 --json --session-dir <tmp> "Summarize the number two."
```

Faux reports zero usage, so the camelCase usage contract is pinned instead by
`fixtures/session/1787544431469_fixture-a.jsonl`, a copy of the committed v4
golden session (`crates/types/tests/fixtures/v4-golden.jsonl`) whose assistant
entry carries real non-zero `input`/`cacheRead`/`cacheWrite` and whose
compaction entry exercises `summarization_count`.
