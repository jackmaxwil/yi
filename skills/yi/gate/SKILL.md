---
name: gate
description: >
  This repository's check recipe: how to run the gate and the focused tests
  so the exit code is the verdict, which nextest filters exist, and which
  tests cannot pass inside the sandbox. Use before running cargo test,
  cargo nextest, cargo clippy or just check, and when a test fails with
  PermissionDenied. Do NOT use to decide whether to run the gate at all: an
  assessment quotes the record instead of running it.
trigger: cargo nextest, cargo test, just check, cargo clippy, PermissionDenied
---

# gate

`gate` → the one command → its exit code → the focused run → the sandbox list

## The gate

`just check` is the gate. It runs fmt-check, clippy with `-D warnings`, the
guardrail scripts and the test suite, and its exit code is the verdict.
Never judge a gate by piped output: `cargo test | grep` reports grep's exit,
not the tests'. Read `$?`.

```
just check; echo "exit $?"
```

For a change you made, run the focused tests first, then the gate. For a
question or an assessment, do not run either; quote the repository's
record (the last merged pull request's CI, the changelog row).

## The focused run

nextest, not `cargo test`, and never `--quiet` (nextest has no such flag;
it fails with `unexpected argument`):

```
cargo nextest run -p <crate>                      # one crate
cargo nextest run -p <crate> -E 'binary(<file>)'  # one integration test file
cargo nextest run -p <crate> -E 'test(<name>)'    # tests whose name contains <name>
cargo nextest run --workspace --no-fail-fast      # everything, all failures listed
```

A bare word after `-p <crate>` is a test-name filter over unit tests only;
an integration test file needs `binary(<file stem>)`. `0 tests run, N
skipped` means the filter matched nothing, not that the tests passed.

Doctests run separately: `cargo test --workspace --doc`.

## Reading a failure

`TRY 2 FAIL` means nextest retried once and it failed again. Run the one
test alone with output:

```
cargo nextest run -p <crate> -E 'test(<name>)' --no-capture
RUST_BACKTRACE=1 cargo nextest run -p <crate> -E 'test(<name>)' --no-capture
```

A test that fails in the full suite and passes alone is a race, not your
diff; read the failing run's own evidence (a frame dump, the session file it
wrote) before the diff.

## Tests that cannot pass inside the sandbox

In auto mode an unprovable command runs contained: no network beyond
loopback, no unix socket, writes only under the working tree and tmp. These tests fail there
for that reason and for no other; the list is
`evals/fixtures/sandbox_bound_tests.txt`:

- `yi-acp daemon::tests::*` bind a unix socket (`bind: Os { code: 1, kind: PermissionDenied }` at `crates/acp/src/daemon.rs`).
- `yi-tools::sandbox` invoke macOS Seatbelt inside a Seatbelt sandbox.
- `yi-console::drive` spawn the daemon and bind its socket.
- `yi-runtime::skills_e2e bundled_python_skills_work_through_the_kernel` reaches no network from the kernel venv.

A PermissionDenied from one of these is a fact about where you ran it. Do
not score the code for it, do not "fix" the test, and do not retry it
inside the sandbox. Say which test, say why, and quote the repository's own
CI for its real state.

## Other facts that cost a turn when guessed

- A sibling session rebuilding `~/.yi/kernel-venv-<hash>` reds kernel tests
  mid-rebuild; wait and retry, never repair the venv.
- A red `test_size`, `crate_size` or `comments` after your change is your
  change; intended growth is a `raise:` line in the change file. A stored
  baseline (`request_budget`) only shrinks, and growth there is `--update`
  in its own commit, never in the code commit.
- `just check` takes minutes; use `wait` up to the clamp and let the
  command become a job if it runs longer, then check it with a bare bash
  call.
