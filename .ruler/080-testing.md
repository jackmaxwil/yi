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
- Tests avoid unwrap/expect by returning `Result<(), Box<dyn Error>>`.
