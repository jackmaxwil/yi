# Declaring work ready

"Done" means all of the following, in order. A passing test suite is
necessary, not sufficient.

1. Build succeeds and the focused tests for the changed path pass.
2. `just check` is green — and every gate is judged by its exit code, never by
   piped output: `cargo test | grep ...` reports grep's exit, not the tests'.
   Chains that swallowed a failing gate have shipped broken commits in this
   repo before.
3. For behavior changes, run the real binary. Offline:
   `./target/debug/yi ask --model faux/faux-1 "<prompt>"` (add `--json` for
   the event stream). Size or startup claims use the dist profile
   (`cargo build --profile dist -p yi-cli`), never debug.
4. Report failures verbatim; never paraphrase an error you have not fixed.

Misdiagnosis defaults: a gate that turns red after your change was broken by
your change — fix the code, never the baseline. If a stale-cache explanation
tempts you, `touch` the crate's lib.rs and rerun before believing it. Ratchet
growth is intentional only as `--update` in its own commit.
