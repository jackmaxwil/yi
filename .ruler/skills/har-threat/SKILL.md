---
name: har-threat
description: Threat modeling for Rust — assets, trust boundaries, STRIDE, agent and tool-output boundaries, untrusted input and per-format deserialization budgets, resource bounds, subprocess containment, secrets and memory residency, AEAD, nonces, key schedules. Load when designing or reviewing a boundary that takes input you do not control.
---

# Threat model

`har` asks whether the code is correct. `har-verify` asks whether it crashes. This asks what the code defends, from whom, across which boundary. A defect has no author; a threat does, and the author picks the input.

## Five steps

One boundary at a time, ten minutes, written down.

1. **Name the assets.** What an attacker wants: a mounted source tree, the user's API keys, an audit log, the ability to run a command as the user. Not "the app" — a specific thing with a specific value.
2. **Enumerate the attack surface.** Every input the process accepts that it did not itself produce. A surface you cannot list is a surface you cannot claim is validated.
3. **Rank.** `risk = likelihood x severity`, both judged against your own system. A one-line panic on a path the attacker feeds outranks a clever exploit needing physical access.
4. **Implement controls at the highest-leverage point** — the parser, the type, the bound. A control applied after parsing is a control an alternate call path skips.
5. **Test the control, not the feature.** If deleting the control breaks no test, the control is decoration.

Re-run step 2 whenever a new input source lands. Boundaries are added by features, not by security work.

## Trust boundaries

A trust boundary is any point where data or control crosses from a component you verify into one you do not. A **design flaw** is a boundary you never drew; a **bug** is a control you drew and implemented wrong. Fuzzing finds the second only.

| Boundary kind | What crosses | Assume |
| --- | --- | --- |
| subprocess stdio | frames from a program you launched | the peer is hostile and the frames are attacker-chosen bytes |
| sandbox or VM edge (socket, vsock, shared mount) | bridged streams, a mounted filesystem | the guest is compromised; the edge is the only control |
| local database | rows written from parsed input | stored data is exactly as trusted as its source — storage is not sanitization |
| embedded webview or template | HTML, JS, URLs another component controls | it executes with the privileges you hand it |
| local socket or named pipe | commands from anything on `PATH` | the caller is not necessarily the program you launched |
| paths on the wire | absolute paths in requests and diffs | traversal, and a symlink swapped between your check and your open |
| a model or agent boundary | text produced by any of the above | data at the process boundary becomes instructions at the model boundary — see below |

Write the boundary list into the repo's architecture document, next to the architecture it describes (`har-layout` owns its shape and naming). An undocumented boundary gets no owner and no review.

## STRIDE

| Letter | Threat | Property violated | Shape at a boundary |
| --- | --- | --- | --- |
| S | Spoofing | authentication | anything on `PATH` speaks your local protocol as the real peer |
| T | Tampering | integrity | a frame edits state it was never given authority over |
| R | Repudiation | non-repudiation | an action with no log row, so no receipt |
| I | Information disclosure | confidentiality | a secret in a `Debug` render, an error, or a span field |
| D | Denial of service | availability | one malformed frame panics the renderer |
| E | Elevation of privilege | authorization | a held effect executes without a verdict |

Walk all six letters per boundary. The letter with no plausible instance is a finding you write down, not one you skip.

Worked example — **a subprocess whose stdout you parse**:

| | Instance | Control | Test that fails when the control is deleted |
| --- | --- | --- | --- |
| S | any binary earlier on `PATH` answers instead of the one you meant | launch by absolute path, resolved once at startup, never through a shell | spawn with a shadowing binary on `PATH`; assert refusal |
| T | a frame names a session id it was never issued | session ids are unguessable and checked against the issuing table, not echoed back | replay a frame with a foreign session id |
| R | the child performs an effect with no record | every accepted frame writes a log row before the effect runs | delete the row-write; an integration test asserting the receipt fails |
| I | child stderr is logged verbatim and contains a token you passed it | never pass secrets in argv or env (below); redact on the log path | log a frame containing a known token pattern; assert redaction |
| D | a 4 GiB line with no newline | `LinesCodec::new_with_max_length` (`har-async`) plus a read bound | feed an unterminated 10 MiB line; assert a bounded error, not an OOM |
| E | a frame requests a path outside the workspace | descriptor-relative open under a pinned root (below) | request `../../etc/passwd` and a symlink swapped mid-flight |

## What Rust does not guarantee

| Not prevented | Security consequence |
| --- | --- |
| memory leaks | attacker-driven growth to OOM — an availability bug |
| deadlock | hang with no crash, so no restart and no alert |
| integer overflow | wrong length or bound; defined wrap, not UB, still a wrong answer |
| panic on a reachable path | process death, i.e. remote DoS from one frame |
| logic errors under `unsafe` | UB, and UB is where exploitation lives |
| timing side channels | key and token recovery; `==` on secret bytes is data-dependent |
| unbounded resource use | see below; the type system has no opinion on size |

`#![forbid(unsafe_code)]` buys memory safety. It buys nothing on this list. Rank UB by how late it surfaces: immediate failure, then corrupted state, then a latent time bomb, then a vulnerability — the loud one is the cheap one.

## Untrusted input

Anything you parse and did not produce is untrusted. Validate inside the parser, so no later call path can reach the value unvalidated, and return a type the rest of the program cannot construct wrong (`har` owns the checked-constructor pattern).

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    session: SessionId,
    #[serde(deserialize_with = "bounded_text")]
    text: String,
}

fn bounded_text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    (s.len() <= 64 * 1024).then_some(s).ok_or_else(|| de::Error::custom("text too long"))
}
```

- Every wire string gets a length bound, every collection a count bound, every nested structure a depth bound. A missing bound is an attacker-chosen allocation.
- Argument parsing carries the same bounds. A hand-rolled parser routes the raw string through the same checked constructor the wire path uses — `Count::new(n)?` — so the CLI and the wire share one validator instead of two that drift. A parser crate can express the bound declaratively instead (`clap`'s `#[arg(long, num_args = 5..=256)]` rejects before your code runs); either way the bound is stated once, in a type.
- `{integer}::strict_*` is **not** a wire-value tool. It panics on overflow in every profile, so on peer input it is a remote denial of service — use `checked_*` and return the error (`har`).
- Never `as` on a wire value; `u32::try_from(x)?` (`har`).
- Data read back out of a local database is still peer data. Re-apply the bound at read, or store only already-validated newtypes.
- Peer text rendered into a webview is script with your app's privileges. Set the text, never build HTML.

### Schema strictness

| Schema | Rule |
| --- | --- |
| Fixed, both ends move together | `#[serde(deny_unknown_fields)]`, and reject duplicate semantic fields |
| Versioned, must accept newer peers | an explicit version discriminator plus **one** allowlisted extension map, bounded in count and bytes, that you never act on |
| Uses `#[serde(flatten)]` | `deny_unknown_fields` is documented as **unsupported** in combination — drop `flatten` at a peer boundary, or hand-write a `Deserialize` visitor that rejects unknown and duplicate keys |

Silently ignored fields are how a version skew becomes a security hole; silently rejected ones are how forward compatibility dies. Unknown-field rejection is also not a grammar: duplicate keys, aliases, defaults, externally tagged enums and the version discriminator each need a stated policy.

### Per-format budgets

"Bound the depth" is a requirement, not an implementation. The parser's own recursion, buffering, aliases and token count must be bounded **before** any allocation, and the knob is per format:

- Put a byte-limited reader in front of every decoder — `Read::take` on the stream, never a check after the buffer filled.
- `serde_json` has a recursion limit; its `unbounded_depth` feature removes it and is banned for peer input unless you supply an independently bounded iterative decoder.
- `serde_cbor` caps depth at 128 internally; other formats differ, and YAML alias expansion is its own bomb class.
- Name the exact decoder constructor and options per allowed format, set collection, string, token and cumulative-allocation budgets, and keep a depth-bomb and an alias-bomb fixture in the regression suite.

`har-verify` rung 5 fuzzes this parser. Write the bound first; the fuzzer proves you meant it.

### Paths

**Canonicalize-then-check is still a TOCTOU bug.** `canonicalize` resolves at one instant; an attacker who can modify the tree swaps a component between your check and your open. std's own `fs` docs call check-then-use TOCTOU-prone and point at atomic operations and retained handles.

Make resolution and open one decision, relative to a directory descriptor you pinned:

- Linux 5.6+: `openat2` with a trusted dirfd and `RESOLVE_BENEATH` (or `RESOLVE_IN_ROOT`), plus `RESOLVE_NO_SYMLINKS` / `RESOLVE_NO_MAGICLINKS` where required, handling `EAGAIN`. `cap-std` is the portable-ish wrapper for this shape.
- Elsewhere: walk component by component on descriptors, holding each open, and refuse anything that is not a directory you already opened.
- `canonicalize` stays useful for normalization and for display. It is never the authorization decision.

## Offensive vs defensive

| Component | Posture | On bad input |
| --- | --- | --- |
| a one-shot CLI | offensive | fail fast: first violation, non-zero exit, message on stderr |
| a long-lived UI or server process | defensive | reject the frame, log the reason, keep the session and the window alive |
| the parser itself | offensive | always — a partial parse is worse than no parse |

Pick one posture per component and write it down. A mixed posture is how a reject path quietly becomes a panic path.

## Resource bounds

Availability is a security property. Every unbounded thing a peer can grow is a denial of service that needs no memory-safety bug.

```rust
use std::io::{BufRead, Read};        // `Read` must be in scope: see below

const MAX_FRAME: u64 = 1 << 20;
const MAX_DRAIN: u64 = MAX_FRAME * 8;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("io")] Io(#[from] std::io::Error),
    /// `resynced` false means the newline was never reached: the stream is
    /// still mid-frame and the caller must close it, not read again.
    #[error("frame over {max} bytes (resynced: {resynced})")]
    FrameTooLong { max: u64, resynced: bool },
}

/// Reads one newline-terminated frame. An over-long frame is drained up to
/// `MAX_DRAIN` so the stream resynchronizes at the next newline rather than
/// returning its tail as a frame. Past that the drain gives up and says so.
fn frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, Error> {
    let mut buf = Vec::new();
    let n = reader.by_ref().take(MAX_FRAME).read_until(b'\n', &mut buf)?;
    match buf.last() {
        None if n == 0 => Ok(None),                       // clean EOF
        Some(b'\n') => { buf.pop(); Ok(Some(buf)) }
        _ => {                                            // hit the cap mid-line
            let mut drain = Vec::new();
            reader.by_ref().take(MAX_DRAIN).read_until(b'\n', &mut drain)?;
            Err(Error::FrameTooLong {
                max: MAX_FRAME,
                resynced: drain.last() == Some(&b'\n'),
            })
        }
    }
}
```

Four things that example is being careful about, and that a hand-rolled bound usually is not. Hitting the cap leaves the reader **mid-line**, so without an explicit resynchronize the next call returns the tail of the oversize frame as a valid one. `read_until` on bytes rather than `read_line` on `String`, because a peer's bytes are not guaranteed UTF-8 and a decode error is not the failure you want here. The drain itself is bounded, because an attacker who sends no newline at all must not get an unbounded read either — **so the resynchronize is best-effort, and the error has to say which it was**: a peer who sends `MAX_DRAIN` bytes with no newline leaves the reader mid-frame, and a caller that treats every `FrameTooLong` as recoverable then resumes at an attacker-chosen offset. Bound the drain and report `resynced: false` as fatal for that connection; an unbounded drain trades the resynchronize bug for the unbounded read. And `take` consumes its receiver, which makes the `use std::io::Read` line load-bearing: with `BufRead` alone, `take` resolves through the supertrait against `impl BufRead` itself and both call sites fail with `E0507`, *cannot move out of `*reader`* — and `by_ref()` does not rescue that, because it resolves the same way. With `Read` in scope, `take` resolves against `&mut R` and reborrows, so the `by_ref()` calls above are belt-and-braces: they say the reborrow out loud and keep the body correct if the signature ever takes the reader by value. On a real protocol, prefer a codec that already does this — `LinesCodec::new_with_max_length` resynchronizes for you, within the same bound (`har-async`).

- Bound every read. `Read::take` on the stream, not a check after the buffer is already full.
- Never size an allocation from a peer's length field. `Vec::with_capacity(n)` from the wire is an OOM primitive; grow as bytes actually arrive.
- Recursion over peer-shaped data carries an explicit depth counter, or becomes iteration with a stack.
- Every wait a peer can hold open gets a timeout: handshake, request turn, connect, subprocess exit.
- Backpressure with a bounded channel. An unbounded queue fed by a subprocess is a leak the subprocess controls.
- Bound retained state too: log rows per session, cached items per view, formatted text runs. "Grows with input" and "bounded" are the whole decision.

## Containing a subprocess

In-process bounds limit what a hostile peer costs you; they do not confine it. A child you launch — or a compromised native dependency — needs a platform profile, and the profile is not portable, so state it per target and **fail closed** when a required primitive is unavailable.

Everywhere:

- Launch by absolute path with an explicit argument vector. Never a shell, never `PATH` resolution at call time.
- Empty or minimal environment, explicit working directory, and close inherited file descriptors.
- Wall-clock, CPU, memory, output-size and file-descriptor limits; a process group so the whole tree dies (`har-async` owns kill and reap).

Linux: `no_new_privs`, a seccomp syscall filter, and Landlock to self-restrict filesystem and (on recent kernels) network rights; cgroups or rlimits for quotas; drop UID and capabilities. macOS: an entitled App Sandbox or XPC service where the architecture allows it.

Test the denials, not just the happy path: a forbidden file open, a network connect, an `exec`, and a fork bomb each get a test that fails if the profile is dropped.

## The agent boundary

When a component feeds tool output, subprocess text, retrieved documents or web content into a language model, STRIDE on the process boundary is necessary and not sufficient. The same bytes are *data* to your parser and *instructions* to the model: that is indirect prompt injection, and no parser can prove natural-language intent.

| Control | Rule |
| --- | --- |
| Provenance | every tool result carries a trust label, and the label travels with it |
| Interpolation | never concatenate tool output into a privileged instruction or system context |
| Text → action | the only route from model output to an effect is a strict typed schema over an allowlisted action set |
| Authorization | a deterministic policy binds user intent, target, capability and spend — decided outside the model |
| Credentials | least-privilege and ephemeral, scoped per tool call, never a long-lived token in the context |
| Confirmation | destructive actions, egress, and privilege changes need explicit human approval |
| Memory | anything persisted and re-read is an injection channel: isolate it, integrity-check it, bound it |
| Circuit breakers | rate, spend and blast-radius caps, plus an audit record per effect |

Adversarial tests belong in the suite: subprocess output that asks for shell execution, that asks for a secret, that claims a policy override, that forges a completion or a tool result.

## Secrets

- A secret is a newtype with a private field, no `Serialize`, no `Display`, and a hand-written `Debug` that prints a placeholder — omitting `Debug` entirely just blocks `derive` on every struct that holds one. `secrecy::SecretBox` / `SecretString` is the reviewed version of exactly this: non-serializing by default, access only through `ExposeSecret`, zeroized on drop. Prefer it unless a local type has tested equivalent behaviour.
- Zero on drop with `zeroize`, and know what that buys. Its guarantee is that the write is not optimized away. It does **not** erase register or stack spills, does not erase bytes left behind by a previous `Vec`/`String` reallocation, and does not touch swap or a core dump. And `Drop` does not run on `abort`, under `panic="abort"`, on SIGKILL, on power loss, or after `mem::forget`. So: treat zeroize-on-drop as ordinary lifetime hygiene, minimize copies and reallocations, and zero buffers explicitly on error paths.
- Above that tier, memory residency is a **deployment** decision, not a crate: suppress core dumps and ptrace attachment (`PR_SET_DUMPABLE` on Linux), decide about locked memory and handle the failure explicitly, and account for swap, hibernation, crash reporters and backups. `secrecy` deliberately does not provide `mlock`/`mprotect`. Check these at startup and either fail closed or degrade loudly.
- Secrets never enter error variants. `har`'s rule that a variant carries the offending value stops at secrets: carry the length and the bound, never the bytes. The error is printed, logged, and pasted into an issue.
- Secrets never enter a log or span field (`har-layout` owns the redaction plumbing) and never enter argv or a child's environment — `ps` and `/proc` are readable by every process on the box. A sandbox edge is worth nothing if the value is passed through as an env var; hand it over a descriptor the child already holds.
- Never `==` on a secret, a MAC tag, or a token. `==` on slices short-circuits at the first differing byte, so the time it takes leaks how long a guessed prefix was correct — which turns forgery from 2^n work into 256n.

  The shape that does not short-circuit is small enough to own, and the length is not secret:

  ```rust
  /// Compares two byte strings without an early exit on the first difference.
  pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
      if a.len() != b.len() { return false; }
      let mut diff = 0u8;
      for (x, y) in a.iter().zip(b) { diff |= x ^ y; }
      diff == 0
  }
  ```

  **Be honest about what that buys.** Rust makes no language-level constant-time guarantee, nothing stops a future optimizer from reintroducing a branch, and `black_box` is a benchmarking barrier that must never be cited for a security property (`har-hot-path`). So this is the correct *shape* and a best-effort implementation: it removes the data-dependent early exit, which is the leak that is actually exploitable over a network. `subtle::ConstantTimeEq` is the reviewed version with the optimizer barriers done deliberately, and for a verification path that an attacker can call in a loop it is worth the dependency (`har-supply`). Either way, write the test that asserts the comparison agrees with `==` on equal, unequal, differing-length and empty inputs — a hand-rolled compare that is wrong is worse than a slow one.

```rust
pub struct Token(String);

impl Drop for Token {
    fn drop(&mut self) { self.0.zeroize(); }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("Token(redacted)") }
}
```

## Crypto usage

### The AEAD contract, and what it does not include

Encryption alone gives you neither integrity nor authenticity. Use an AEAD (`aes-gcm`, `chacha20poly1305`); an untagged ciphertext is attacker-editable in place. Four things an AEAD does **not** give you, each needing its own rule:

**Replay.** An attacker resends a valid frame without decrypting anything, and it verifies. Putting a counter or session id in the associated data *authenticates* that value — it does not reject the replay. The receiver must hold freshness state: after successful authentication, check the counter against a persisted monotonic value or a bounded anti-replay window, scoped to the session and key epoch. Specify the counter's overflow and rekey behaviour, and test duplicate, reordered, stale-session, post-restart, counter-wrap and concurrent delivery.

**Key commitment.** Conventional AES-GCM, ChaCha20-Poly1305 and AES-GCM-SIV do not commit a ciphertext to a single key: one ciphertext can verify under two chosen keys with different plaintexts. That matters the moment there is more than one candidate key — envelope or multi-recipient encryption, key rotation, an attacker-supplied key id. Use a committing construction or authenticate an independent key commitment, and never make "the first key that decrypts" an identity decision.

**Context binding.** Accepting an unauthenticated algorithm id, key id, protocol version or recipient context is a cross-context and downgrade primitive. Authenticate the suite id, protocol version, key epoch, role or direction, recipient, and message type.

**Failure handling.** Nothing may leave the boundary before authentication succeeds: do not parse, act on, log, or retain an in-place buffer on an inauthentic ciphertext, and wipe the mutable plaintext buffer on error. Return one externally uniform authentication failure. Forgery probability grows with attempts, so cap and rate-limit verification failures per key, session and source — NIST asks for exactly that monitoring.

### Nonces

Nonce reuse under one key breaks the construction outright — WPA2 KRACK, the PS3 ECDSA key recovery. `har` carries the enforcement: distinct `EncryptNonce` / `DecryptNonce` types taken by move.

| Construction | Nonce | Budget, and where it comes from |
| --- | --- | --- |
| AES-256-GCM, **random** IV | 96-bit | NIST SP 800-38D §8.3: at most **2^32 authenticated-encryption invocations per key**, counting every invocation under that key at any IV length, with IV-collision probability ≤ 2^-32 |
| AES-256-GCM, **deterministic** 96-bit IV | 96-bit | the fixed and invocation fields must themselves guarantee uniqueness — persist and partition them, reject overflow, rekey on unrecoverable state loss |
| XChaCha20-Poly1305, random nonce | 192-bit | ~2^80 is the XChaCha Internet-Draft's collision calculation at a 2^-32 target — a budget from an expired draft, not a standards cap |

GCM permits IV lengths other than 96 bits; 96 is the recommended form, not the only one, and the 2^32 bound is not specific to it. Read the table as: **random nonces mean a durable, global, per-key invocation counter and a rekey before exhaustion.** Deterministic counters remove the collision question and add a durable-state question instead. XChaCha's large nonce makes random generation comfortable and still requires uniqueness; choosing it defers the counting obligation rather than deleting it.

Generate keys and nonces from an RNG the type system says is cryptographic, never one that merely implements the core trait — the core trait alone admits a small fast PRNG, which is reproducible and predictable. **The exact bound is version-specific and has changed twice**, so read the pinned version's own `CryptoRng` page and write it from that rather than carrying this line across an upgrade: in `rand` 0.10 `CryptoRng` is an alias for `Rng<Error = !> + TryCryptoRng`, so `R: CryptoRng` is the whole bound and `TryCryptoRng` is the fallible form; 0.9 had `CryptoRng: RngCore`; 0.8 needed `R: RngCore + CryptoRng`. (`har-io` owns entropy availability on constrained targets.)

### Key schedule

One key per purpose, derived — never one key reused across encryption, MAC, wrapping, directions, tenants or epochs.

RFC 5869 HKDF is extract-then-expand. Do not skip extract on a non-uniform input such as a Diffie-Hellman shared secret. The `info` field is what separates contexts and it is not optional in practice:

```rust
let hk = Hkdf::<Sha256>::new(Some(&salt), &shared_secret);      // extract
let mut key = Zeroizing::new([0u8; 32]);
hk.expand(&context, &mut *key)?;                                 // expand
```

Build `context` canonically and length-delimited, carrying protocol name, version, role or direction, algorithm id, tenant or session, and key epoch — so two different contexts can never serialize to the same bytes. Derive separate AEAD, header/commitment and exporter keys from it. Write down rotation, compromise response, deletion, and how the key epoch couples to nonce state.

HKDF adds no entropy and no work factor, so it is **not** a password KDF. Passwords go through a memory-hard KDF — Argon2id — with a unique salt and calibrated parameters.

### Agility and inventory

Hard-coding today's suite with no inventory makes tomorrow's replacement unsafe, and unauthenticated negotiation makes it attacker-driven. Keep a cryptographic inventory; put an **authenticated** suite, key-format and key-epoch identifier in a versioned envelope; allow only a policy allowlist with a minimum version and downgrade resistance; keep known-answer vectors per suite; and give migration paths an expiry. Symmetric AES-256 is not the urgent part — the post-quantum transition is about key establishment and signatures, so plan a hybrid path from a standard rather than inventing one.

Decide which attacker first: MITM owns the wire, MATE owns the machine. Nothing shipped in a binary defeats MATE — obfuscation buys time, not secrecy. A sandbox or VM edge is a MITM-class control against the component inside it, not a MATE defense against the user.

## What tools cannot find

Three vulnerability classes sit outside every rung of `har-verify`: improper input validation, information leakage, and misconfiguration. Rice's theorem is the reason the general case is not decidable, but the practical reason is simpler — a backdoor gated on a magic token survives 100% line coverage and a week of fuzzing, because random search will not guess an 11-character trigger. Tools scale review; they do not replace the model. The ladder is aimed at defects, so aim this at adversaries separately.

## Boundary review checklist

- Assets named, and the boundary drawn in the architecture document in prose someone else can read
- Every input enumerated; each one bounded in length, count and depth **at its parser**, with the decoder's own options named
- All six STRIDE letters walked, including the ones you dismissed and why
- Failure posture stated: fail fast or degrade, and consistent across the component
- Every peer-controlled wait has a timeout; every peer-fed queue is bounded; over-long frames resynchronize rather than truncating
- Subprocesses launched by absolute path with a minimal environment, a stated isolation profile, and tested denials
- Paths opened descriptor-relative under a pinned root, not canonicalize-then-check
- No secret in a `Debug` render, an error variant, a log field, argv, or a child's environment; memory-residency posture stated
- AEAD in use with replay state, key commitment where more than one key exists, authenticated context, and uniform failure handling
- Nonce budget written down with its source; keys derived through a context-separated schedule, passwords through Argon2id
- Tool and model output treated as untrusted at the model boundary, with a typed action allowlist and human approval for destructive effects
- The control has a test that fails when the control is deleted
- New dependency at this boundary triaged by `har-supply` before it lands
