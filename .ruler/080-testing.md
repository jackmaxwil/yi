# Testing doctrine

Every test defends one concrete, externally observable contract. Name the
failure mode a consumer would see if it regressed; if you cannot name one, do
not add the test.

- External ground truth over self-confirmation: a round-trip proves
  reversibility, not correctness. Fixtures come from the reference
  implementations themselves (Pi-generated session files, canned provider SSE
  transcripts), never from Yi's own output.
- Assert exact bytes and ordering only where a consumer parses the exact bytes
  (wire fixtures, JSONL); otherwise assert semantic content.
- Never source-grep: a test that reads source text and asserts on it tests how
  code looks, not what it does.
- No static echoes, passthrough assertions, or "the code ran" tests.
- Provider behavior tests need no keys: the faux provider replays scripted
  event streams; drive mappers with canned payloads.
- A new gate is proven the same way a fix is: disable the gate (not the test) and watch the
  test fail for the gate's own reason — the wall's deny was verified by neutering `Wall` in the
  adapter and seeing the denied command's marker file appear.
- A regression test is run against the unfixed code before the fix is claimed:
  temporarily revert the fix, watch it fail, restore. Two tests written this
  way passed against the broken code on the first try — one modelled a
  terminal reflow the emulator does not perform, the other asserted a shape
  the fix had already made unreachable. A green test proves nothing until it
  has been seen red for the right reason.
- Tests avoid unwrap/expect by returning `Result<(), Box<dyn Error>>`.
