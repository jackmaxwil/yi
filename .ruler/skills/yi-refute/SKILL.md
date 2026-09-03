---
name: yi-refute
description: Prove a test or gate red for its own reason before claiming it: fixture shape, seen-red run, mutants
---

# Refuting a test before claiming it

1. Name the failure a consumer would see. If the fixture you are about to type is not the
   smallest input that produces it in the field, stop: find the session that did
   (`python3 skills/yi/session-mining/extract.py`, fingerprint by tool error) and scrub it with
   `evals/record.py <session> --scrub-only --out crates/tui/tests/fixtures/sessions/<id>.jsonl`.
2. Write the life-sized case over that artifact and the degenerate case by hand (one
   unterminated paragraph, a `30` where a dict is due, a non-ASCII string at a byte index).
3. Run the test against the unfixed code: revert the hunk. Copy the failure line; it is the
   PR's `Seen red` entry. A green here is a static echo — rewrite the assertion, not the fixture.
4. Restore the fix; run the focused test, then `just check`, reading `$?` not grep.
5. If the test pins a user-facing sentence, render the surface (`row_text`, a drive script) and
   read the sentence beside its neighbours. Quote it in User outcomes as the user sees it.
6. If the change adds a `Cell` family, name its antecedent in tui_unit.rs's match and write the
   pair test where the dependent finishes first.
7. For a new gate: disable the gate, not the test, and watch it fail for the gate's reason;
   a new guardrail script's `--selfcheck` disables each of its checks in turn and must fail for
   that check's own reason (check_orphans.py is the shape).
8. Optional, for a renderer or parser fix: `cargo mutants --in-diff <(git diff main) -p <crate>`
   (user-run, minutes; never in a gate until its wall clock is measured); quote survivors in
   `Seen red`, or kill them.
9. Report failures verbatim; a paraphrase is a claim.
