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
- A fixture is the production shape, never the convenient one: the smallest input that fails in
  the field. One unterminated paragraph, not three newline-terminated ones; a forty-nine-row
  kernel traceback, not two; a schema that is `30`, not data that misses a valid schema; a
  non-ASCII string where a byte index is taken. Every renderer and parser test carries one
  life-sized case and one degenerate case, and the life-sized one is a recorded artifact — a
  scrubbed session under crates/tui/tests/fixtures/sessions/, written by
  `evals/record.py --scrub-only` — fed through the reducer the screen runs. A session is ground
  truth for input shape; the expected output still comes from the invariant, never from what Yi
  rendered. New sessions are added beside the old, never edited. Fourteen defects sat beside
  green tests whose fixtures had chosen the shape that works.
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
- A test that pins a user-facing sentence has read it beside what the user sees next to it.
  "completed without replying" was asserted on the row above "Last answer: …", and the test
  defended the contradiction. Quote the sentence in the PR's User outcomes, in its context.
- Every cell family that commits to scrollback names its antecedent in tui_unit.rs's exhaustive
  match and has a pair test where the dependent finishes first. Two commit clocks kept correct
  time alone and were never run against each other — a child landed above the cell that
  spawned it.
- The seen-red run is written down: the PR's `Seen red` section carries, per new test, the
  failure it produced against the unfixed code and where the fixture came from.
- Tests avoid unwrap/expect by returning `Result<(), Box<dyn Error>>`.
- Attribute a red gate before editing anything: a test that fails inside the full suite and
  passes standalone is a race, not your diff. Read the failing run's own evidence (frame dumps,
  the session file it wrote) rather than the diff, and fix the race — `wait-idle` walking on
  before a submitted turn had started was A10, and the frames said so.
- A drive script waits on the state it depends on, never on a duration: a prompt reaches the
  runtime thread over a channel, so `running` is still false the instant after Enter.
- The kernel-side Python has its own lane: python/yi_runtime/tests runs in check_guardrails.sh,
  stdlib unittest over faked `list_subagents`/`result`. 1,789 lines had no test because every
  test in the suite `just check` runs called the host directly; only a tier-2 journey reached
  the shim.

Tests are tiered by where they run and what they may spend:

- **T0** unit and contract tests — `just check`, zero spend.
- **T1** faux cassettes: behavior baseline, plan/goal/permission e2e over `faux/faux-1` —
  `just check`, zero spend.
- **T2** real-binary journeys: the built `yi` driven end to end, offline. Zero spend. A cheap one
  rides `just check` too; one that sleeps or boots a process tree per assertion is `#[ignore]`d
  with a `tier-2 journey` reason and runs only in `just journeys`, which `just postmerge` and the
  CI postmerge lane depend on — the attribute is the lane, so the test itself says where it runs,
  and `check_test_tiers.py` holds that reason string exact so nothing that spends money can be
  written into the lane a push to main runs.
- **T3** paid smoke: one real-provider run per suite, ledgered in docs/eval-ledger.md with its
  config fingerprint. Opt-in and user-run; money in a gate is what plan law 3 forbids, so T3
  never sits in one.
- Every feature-ledger row names its journey test in a `journey:` clause. A row that cannot name
  the test a user-visible regression would trip is asserting "live" on vibes; a new or edited
  row without the clause is not done.
