---
name: har-io
description: Correctness at the edges of a Rust process — durable file writes and fsync, atomic replace, database transactions and retry classes, monotonic vs wall clock, deadlines and clock anomalies, CSPRNG and entropy failure, UTF-8 vs graphemes, normalization and identifier comparison. Load when a write must survive a crash, when code reads a clock or an RNG, or when text is sliced, truncated, or compared.
---

# The edges of the process

`har` makes the in-memory value correct. This file covers the four places where a correct value still produces a wrong system: the write that did not reach the disk, the clock that went backwards, the randomness that was not random, and the text that was sliced through a character.

# Durability

## Drop is not a commit

`File`'s `Drop` closes the descriptor and **discards any error**, and a successful `write` only means the kernel accepted the bytes. Neither is a promise that anything reached stable storage.

So every externally visible write states four things before it is written: **where the commit point is**, what the caller may assume before and after it, what happens on a crash between them, and how a retry is distinguished from a duplicate.

```rust
fn commit(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;                       // the data, and its metadata, are durable
    drop(f);
    fs::rename(&tmp, dir.join(name))?;   // atomic replace within one filesystem
    File::open(dir)?.sync_all()?;        // the *directory entry* is durable too
    Ok(())
}
```

Four things that example is being careful about:

- `sync_all` (or `sync_data` when only the bytes matter) is the commit point. Without it the rename can land while the contents have not.
- The rename is atomic **within one filesystem**; across a mount boundary it is a copy and the atomicity is gone.
- Syncing the file is not enough — the directory entry that names it needs its own sync, or a crash can leave the new name pointing at nothing.
- The temporary file is in the destination directory, not `/tmp`, for exactly that reason.

Where the platform gives you an atomic primitive, use it instead of a check: `File::create_new` fails if the path exists rather than racing between your `exists()` and your `create()`. `har-threat` owns the rest of the path-TOCTOU story, and it applies here — hold the open handle across the operation rather than re-resolving a path.

Partial writes are ordinary, not exotic: `write` may accept fewer bytes than you gave it, which is why `write_all` exists, and two processes appending to one file interleave unless the platform's append semantics say otherwise. Never assume one `write` call is one record.

## Databases

A transaction rolled back by `Drop` is a transaction whose failure nobody saw.

- Scope the transaction to the smallest unit that must be atomic. A transaction held open across a network call or a user interaction is a lock held across a network call.
- **`commit()` explicitly, and handle its error.** A failed commit is not a rollback you can ignore; it is an outcome you do not know.
- Say what the isolation level is and what anomaly it permits. "The default" is not an answer if two writers touch the same row.
- Decide the retry class per statement — serialization failure and deadlock are retryable, a constraint violation is not — and make every retryable write idempotent with a caller-supplied key, because a timeout leaves you unable to tell "never ran" from "committed and the reply was lost".
- Test crash recovery, not just rollback: kill the process between the write and the commit, restart, and assert the invariant.

## Model the outcome, not the call

The single most useful shape here is an outcome type that distinguishes what a `Result<(), io::Error>` cannot:

```rust
pub enum Outcome {
    NotStarted,        // safe to retry: nothing was attempted
    Committed(Receipt),
    Failed(Error),     // definitely did not take effect; safe to retry
    Unknown(Error),    // timed out or the connection dropped mid-flight — reconcile, never blind-retry
}
```

`Unknown` is the variant people leave out, and it is the one that causes double charges and duplicate rows. `har-async` owns why a `timeout` produces it.

# Time

## Two clocks, two jobs

| Need | Clock | Failure mode to handle |
| --- | --- | --- |
| Elapsed time, deadlines, backoff, rate limits, a lease local to this process, benchmarking | `Instant` — monotonic | none, but it is meaningless across processes or reboots |
| A timestamp a human or another system will read: log lines, file times, audit records | `SystemTime` — wall clock | jumps forward, jumps **backward**, and is not unique |

Never use one for the other's job. A deadline computed from `SystemTime` misfires when NTP steps the clock; an audit record stamped with `Instant` is meaningless outside the process.

**Wall time does not increase.** `SystemTime::duration_since` and `elapsed` return `Result` precisely because the answer can be negative, and that `Result` is not noise to `unwrap` — decide whether a backwards jump means clamp, reject, or re-read from an authority. Use checked arithmetic wherever a representability failure must be visible rather than saturating silently.

Do not derive security or ordering from a local wall clock without saying how skew, leap seconds and rollback are handled: token expiry, distributed lease validity, and "which write won" all need a server-side or monotonic authority. `har-threat` owns the consequence when an attacker can influence the clock.

**A deadline composes; a duration does not** — compute `Instant::now() + budget` once at the entry point and pass the `Instant` down (`har-async` shows the async form).

Take the clock as a dependency, not a global, so boundary conditions are testable:

```rust
pub trait Clock { fn now(&self) -> Instant; fn wall(&self) -> SystemTime; }
```

Then test the rollback, the step forward, the deadline that expires exactly on the boundary, and the race between a timeout and a completion.

# Randomness

Randomness is a **deployment capability with a failure mode**, not a helper call.

- Keys, nonces, salts, tokens, randomized protocol fields and hash-flood resistance all come from the OS or platform CSPRNG — `getrandom`, or a `rand` RNG bounded by `CryptoRng` (`har-threat` owns the bound and why `RngCore` alone is not it).
- **Propagate entropy failure and fail closed.** The substitutes an agent reaches for when the call is fallible — a timestamp, a counter, `DefaultHasher`, a seeded PRNG — are each a complete break of whatever the value was protecting.
- Pin an approved backend for **every supported target**. `no_std`, Wasm and embedded targets do not all have one, and `getrandom` needs an explicitly registered custom source on some of them. A target with no reviewed hardware or OS entropy source is a target you return "unsupported" for, not one you paper over.
- Keep deterministic seeded RNGs — tests, simulation, property shrinking — in a type distinct from the production CSPRNG, so the two can never be swapped by an import. Never log a seed or secret random material.
- Random generation does not discharge a nonce-uniqueness obligation; that is a per-key allocation policy (`har-threat`).

# Text

`&str` guarantees valid UTF-8. It guarantees nothing about characters, and almost every text bug lives in that gap.

## Boundaries

- **Never index or slice a `str` by a byte offset you did not get from the string itself.** `&s[..n]` panics mid-character; `har`'s panic budget applies. Use `char_indices`, `floor_char_boundary`/`ceil_char_boundary`, or `get(..n)` when the offset came from arithmetic.
- Truncating for display or for a length limit is a **grapheme** operation, not a byte or `char` one: an emoji with a skin-tone modifier, or a base letter plus a combining accent, is several `char`s and one thing the user sees. Use a segmentation crate (`unicode-segmentation`) when the boundary is user-visible — characters, words, lines, cursor positions, "at most 20 characters".
- Byte length, scalar count and grapheme count are three different numbers. A limit states which one it means. A wire bound is bytes (`har-threat`); a UI limit is graphemes; `chars().count()` is almost never what anyone wanted.

## Ingress and comparison

- Convert and validate at ingress, and make the invalid-text behaviour explicit: reject, or replace with U+FFFD, chosen per boundary — `String::from_utf8` versus `from_utf8_lossy` is that decision, and silently lossy conversion at a security boundary hides an attack.
- **Comparison policy is per protocol.** Say whether identifiers are compared by exact bytes, after a named Unicode normalization form (NFC is the usual choice), or with case folding — and apply it in exactly one place. Two strings that look identical can differ in bytes; two that differ visually can fold together.
- Keep the raw identifier and the display string as separate values wherever authorization or logging is involved. Authorize on the raw form, display the other, and never let a normalization step feed the authorization decision.
- Confusables, bidirectional control characters and combining marks are display-layer attacks: a name rendered right-to-left can read as something else entirely, including in a log or a terminal. Strip or escape bidi controls in anything you print.
- Locale-sensitive formatting, collation and the Unicode version itself are dependencies with versions. Pin them and keep multilingual, confusable and combining-character cases in the regression suite.

# Checklist

- Every externally visible write names its commit point, and calls `sync_all`/`sync_data` where durability is claimed
- Replace is temp-file-plus-rename within one filesystem, with the directory synced too
- No check-then-create; atomic primitives and retained handles instead
- Every transaction commits explicitly and handles the commit error; retry classes and idempotency keys are stated
- Timeouts produce an `Unknown` outcome that is reconciled, never blind-retried
- `Instant` for elapsed and deadlines, `SystemTime` for timestamps, and no `unwrap` on `duration_since`
- The clock is injectable, and rollback and skew have tests
- Every random value comes from a CSPRNG whose failure is propagated; every supported target has a reviewed entropy source
- Test RNGs are a distinct type from production ones
- No byte-offset slicing of `str` that did not come from the string; user-visible limits are grapheme limits
- Invalid UTF-8 behaviour chosen per ingress point; identifier comparison policy named and applied once
