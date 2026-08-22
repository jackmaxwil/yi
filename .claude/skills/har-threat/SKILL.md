---
name: har-threat
description: Threat modeling for Rust — assets, trust boundaries, STRIDE, untrusted input at a parser, resource bounds, secrets, AEAD and nonce discipline, and what Rust does not guarantee. Load when designing or reviewing a boundary that takes input you do not control.
---

# Threat model

`har` asks whether the code is correct. `har-verify` asks whether it crashes. This asks what the code defends, from whom, across which boundary. A defect has no author; a threat does, and the author picks the input.

## Five steps

One boundary at a time, ten minutes, written down.

1. **Name the assets.** What an attacker wants: the repo mount, the user's API keys, the SQLite log, the ability to run a command as the user. Not "the app" — a specific thing with a specific value.
2. **Enumerate the attack surface.** Every input the process accepts that it did not itself produce. A surface you cannot list is a surface you cannot claim is validated.
3. **Rank.** `risk = likelihood x severity`, both judged against your own system. A one-line panic on a path the attacker feeds outranks a clever exploit needing physical access.
4. **Implement controls at the highest-leverage point** — the parser, the type, the bound. A control applied after parsing is a control an alternate call path skips.
5. **Test the control, not the feature.** If deleting the control breaks no test, the control is decoration.

Re-run step 2 whenever a new input source lands. Boundaries are added by features, not by security work.

## Trust boundaries

A trust boundary is any point where data or control crosses from a component you verify into one you do not. A **design flaw** is a boundary you never drew; a **bug** is a control you drew and implemented wrong. Fuzzing finds the second only.

| Boundary | What crosses | Assume |
| --- | --- | --- |
| agent subprocess stdio | ACP v2 JSON-RPC frames | the agent is hostile and the frames are attacker-chosen bytes |
| microVM edge (vsock, `agentd`) | bridged stdio, repo mount | guest is compromised; the edge is the only control |
| rusqlite log + frame state | rows written from parsed agent output | stored data is exactly as trusted as its source — storage is not sanitization |
| webview content | HTML, JS, URLs the agent or a page controls | it executes with the privileges you hand it |
| CLI socket | canvas verbs from anything on `PATH` | the caller is not necessarily the agent you launched |
| paths on the wire | absolute paths in tool calls and diffs | traversal; a symlink is resolved after your prefix check unless you canonicalize first |

Write the boundary list into `SUMMARY.md` next to the architecture it describes. An undocumented boundary gets no owner and no review.

## STRIDE

| Letter | Threat | Property violated | Shape at a boundary |
| --- | --- | --- | --- |
| S | Spoofing | authentication | anything on `PATH` speaks CLI verbs as the agent |
| T | Tampering | integrity | a frame edits state it was never given authority over |
| R | Repudiation | non-repudiation | an action with no log row, so no receipt |
| I | Information disclosure | confidentiality | a secret in a `Debug` render, an error, or a span field |
| D | Denial of service | availability | one malformed frame panics the renderer |
| E | Elevation of privilege | authorization | a held effect executes without a verdict |

Walk all six letters per boundary. The letter with no plausible instance is a finding you write down, not one you skip.

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
- `deny_unknown_fields` on every type deserialized from a peer, unless the wire format is versioned and must accept newer peers — then capture the remainder in one `HashMap<String, Value>` you bound and never act on. Silently ignored fields are how a version skew becomes a security hole; silently rejected ones are how forward compatibility dies.
- `clap` carries the same bounds in the type: `#[arg(long, num_args = 5..=256)]` rejects before your code runs.
- Never `as` on a wire value; `u32::try_from(x)?` (`har`).
- Canonicalize a path from the wire, then check the prefix. Checking then canonicalizing is the traversal bug.
- Data read back out of SQLite is still agent data. Re-apply the bound at read, or store only already-validated newtypes.
- Agent text rendered into a webview is script with your app's privileges. Set the text, never build HTML.

`har-verify` rung 4 fuzzes this parser. Write the bound first; the fuzzer proves you meant it.

## Offensive vs defensive

| Component | Posture | On bad input |
| --- | --- | --- |
| `afterlife` CLI, one-shot | offensive | fail fast: first violation, non-zero exit, message on stderr |
| canvas process, long-lived | defensive | reject the frame, log the reason, keep the session and the window alive |
| the parser itself | offensive | always — a partial parse is worse than no parse |

Pick one posture per component and write it down. A mixed posture is how a reject path quietly becomes a panic path.

## Resource bounds

Availability is a security property. Every unbounded thing a peer can grow is a denial of service that needs no memory-safety bug.

```rust
const MAX_FRAME: u64 = 1 << 20;

fn frame(reader: &mut impl BufRead) -> Result<Option<String>, Error> {
    let mut line = String::new();
    match u64::try_from(reader.take(MAX_FRAME).read_line(&mut line)?)? {
        0 => Ok(None),
        MAX_FRAME => Err(Error::FrameTooLong { max: MAX_FRAME }),
        _ => Ok(Some(line)),
    }
}
```

- Bound every read. `Read::take` on the stream, not a check after the buffer is already full.
- Never size an allocation from a peer's length field. `Vec::with_capacity(n)` from the wire is an OOM primitive; grow as bytes actually arrive.
- Recursion over peer-shaped data carries an explicit depth counter, or becomes iteration with a stack.
- Every wait a peer can hold open gets a timeout: handshake, prompt turn, vsock connect, subprocess exit.
- Backpressure with a bounded channel. An unbounded queue fed by a subprocess is a leak the subprocess controls.
- Bound retained state too: log rows per session, frames per zone, cached text runs. "Grows with input" and "bounded" are the whole decision.

## Secrets

- A secret is a newtype with a private field, no `Serialize`, no `Display`, and a hand-written `Debug` that prints a placeholder — omitting `Debug` entirely just blocks `derive` on every struct that holds one.
- Zero on drop. `zeroize::Zeroize`, or `#[derive(ZeroizeOnDrop)]`.
- Secrets never enter error variants. `har`'s rule that a variant carries the offending value stops at secrets: carry the length and the bound, never the bytes. The error is printed, logged, and pasted into an issue.
- Secrets never enter a log or span field (`har-layout` owns the redaction plumbing) and never enter argv or the guest's environment — `ps` and `/proc` are readable by every process on the box. Afterlife keeps secrets host-side of the microVM edge; that edge is worth nothing if the value is passed through as an env var.
- Compare with `subtle::ConstantTimeEq`, never `==`. Early-exit comparison leaks the prefix length.

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

- Encryption alone gives you neither integrity nor authenticity. Use an AEAD (`aes-gcm`, `chacha20poly1305`); an untagged ciphertext is attacker-editable in place.
- AEAD does not stop replay. An attacker resends a valid frame without decrypting anything. Bind a counter or session id into the associated data, or keep a replay window.
- Nonce reuse under one key breaks the construction outright — WPA2 KRACK, the PS3 ECDSA key recovery. `har` carries the enforcement: distinct `EncryptNonce` / `DecryptNonce` types taken by move.
- Generate keys and nonces from an RNG bounded by `CryptoRng`, never merely `RngCore` — `RngCore` alone admits `SmallRng`, which is fast, reproducible, and predictable. In `rand` 0.9 `CryptoRng: RngCore`, so `R: CryptoRng` is the whole bound; on `rand` 0.8 write `R: RngCore + CryptoRng`.

| Construction | Nonce | Random-nonce messages per key |
| --- | --- | --- |
| AES-256-GCM | 96-bit | ~2^32 (4.3e9) — NIST SP 800-38D caps random-nonce invocations here |
| XChaCha20-Poly1305 | 192-bit | ~2^80 — effectively unbounded |

If the message count is unbounded or merely unknown, take the 192-bit nonce. Choosing XChaCha20-Poly1305 deletes the counting obligation rather than deferring it.

Decide which attacker first: MITM owns the wire, MATE owns the machine. Nothing shipped in a binary defeats MATE — obfuscation buys time, not secrecy. Afterlife's microVM edge is a MITM-class control against the agent, not a MATE defense against the user.

## What tools cannot find

Three vulnerability classes sit outside every rung of `har-verify`: improper input validation, information leakage, and misconfiguration. Rice's theorem is the reason the general case is not decidable, but the practical reason is simpler — a backdoor gated on a magic token survives 100% line coverage and a week of fuzzing, because random search will not guess an 11-character trigger. Tools scale review; they do not replace the model. The ladder is aimed at defects, so aim this at adversaries separately.

## Boundary review checklist

- Assets named, and the boundary drawn on a diagram or in prose someone else can read
- Every input enumerated; each one bounded in length, count, and depth at its parser
- All six STRIDE letters walked, including the ones you dismissed and why
- Failure posture stated: fail fast or degrade, and consistent across the component
- Every peer-controlled wait has a timeout; every peer-fed queue is bounded
- No secret in a `Debug` render, an error variant, a log field, argv, or the guest environment
- AEAD in use, nonces from a `CryptoRng`, replay addressed explicitly
- The control has a test that fails when the control is deleted
- Deserialized types reject unknown fields; paths canonicalized before the prefix check
- New dependency at this boundary triaged by `har-supply` before it lands
