# anydoc and pdf-inspector: `read` on every format, without the crate

```
status:  IMPLEMENTED in 0.207.0 (D171, #368), all three phases. The §9
         trigger for phases 2-3 was not met and was waived by the owner on
         2026-09-10. Where the build departs from this text, §12 says how.
date:    2026-09-09
inputs:  github.com/firecrawl/anydoc (crates.io `anydoc` 0.2.4, PyPI
         `firecrawl-anydoc` 0.2.4, MIT) · github.com/firecrawl/pdf-inspector
         (PyPI `pdf-inspector` 1.19.0, MIT) · YI_DESIGN §14.6's demotion row
         and §1.1's one-in-one-out · this tree at ebbfba3:
         crates/tools/src/hashline/tool.rs:378-390, crates/tools/src/exec.rs,
         crates/kernel/src/bootstrap.rs:10-27 · guardrails/binary_size_budget.json
         · 410 sessions under ~/.yi/sessions · four dist-profile builds and
         four real-document conversions run for this document
ruling:  the number decides the rung, and the sessions decide whether there
         is a rung at all. The in-process Rust crate is refused on a measured
         +3.42 MiB against 401 KB of headroom. The engine is two lines in the
         kernel venv's extras array. The tool surface is `read`, or nothing —
         a new tool the model must learn is not admitted (D117). Nothing here
         makes a network call: hosted OCR is refused outright, not defaulted
         off (§6).
```

## 1. The standing ruling this has to answer to

Document conversion was already designed, priced and **demoted**. YI_DESIGN
§14.6 records it:

> Document conversion (`anydoc` behind a `docs` cargo feature; was M1, §14.6):
> demoted 2026-08-31 as the one-out for the console (D89). A format converter
> is not a coding agent's own capability, the kernel venv already reaches every
> converter Python has, and the feature would have carried
> `pdf-inspector`/`zip`/`quick-xml`/`cfb`/`lopdf` plus a fourth cargo gate into
> a build D36 and D38 spent effort collapsing. **Rebuild case: `read` of office
> and PDF files is measured to be a real loop the kernel cannot close.**

So this proposal has exactly two honest routes:

1. **Meet the rebuild case** — measure the loop, then re-admit a top-level
   feature, which under §1.1 means demoting something else and editing that
   list in the same commit.
2. **Add no top-level feature** — no new crate, no new cargo gate, no new
   tool, no new turn participant — and therefore need no one-out at all.

This proposal takes route 2, and §9 reports what the measurement of route 1
actually says. Every number below was produced for this document, not
inherited.

## 2. What the two projects are, measured

`anydoc` converts 14 formats across 8 file types to GitHub-flavoured Markdown:
Word (`.doc`, `.docx`, `.docm`), PowerPoint (`.ppt`, `.pps`, `.pot`, `.pptx`,
`.pptm`, `.ppsx`, `.ppsm`), Excel (`.xls`, `.xlsx`, `.xlsm`, `.xlsb`),
OpenDocument (`.odt`, `.ods`, `.odp`), `.rtf`, `.epub`, `.csv`, `.pdf`. Format
comes from content markers, not the extension. `pdf-inspector` is what it uses
for PDFs, and separately classifies a PDF as text-based, scanned, image-based
or mixed.

Both are pure Rust with Python, Node and WASM bindings. Only one `-sys` crate
appears in the whole graph (`core-foundation-sys`, Apple), so there is no C
toolchain and no `cc` in the build — the pure-Rust claim holds.

### 2.1 The in-process Rust crate, priced

| measurement | value |
|---|---|
| `cargo add anydoc` on an empty lib | 146 packages locked, **103 unique crates** in `cargo tree -e normal` |
| direct deps of `anydoc` | `cfb`, `csv`, `encoding_rs`, `flate2`, `log`, `pdf-inspector`, `quick-xml`, `zip` (8) |
| `--no-default-features` | identical graph — there is nothing to trim |
| Yi's graph today | 18 direct / 166 transitive |
| dist-profile bin, do-nothing baseline | **286,096 bytes** |
| dist-profile bin, same code + a live `anydoc::to_markdown_bytes` call | **3,871,472 bytes** |
| **delta** | **+3,585,376 bytes (+3.42 MiB)** |
| `guardrails/binary_size_budget.json` | `max_bytes: 6,097,824` |
| current dist binary | ~5,696,544 → **401,280 bytes of headroom** |

The probe used Yi's exact `[profile.dist]` (`opt-level = "z"`, fat LTO, one
codegen unit, panic abort, symbols stripped) and its baseline reproduces the
size ledger's own do-nothing figure to within 48 bytes, so the two are
comparable. The probe **calls** the converter and prints its result, which is
the correction the ledger's D74 row paid for: a probe that only links a library
lets LTO dead-strip the parser and understates it by megabytes.

The delta is **8.9× the remaining headroom**. It is not a close call, and a
`docs` cargo feature does not save it: the gate that is off ships dead code and
a second CI matrix leg, and the gate that is on blows the ceiling. D36 and D38
spent real effort collapsing cargo gates; this would be the fourth.

One more disqualifier, independent of size: `chrono v0.4.45` is in the graph,
and §13.5 bans `chrono` by name (H1's schedule is in UTC because of it).
`time`, `aes` and the `unicode-*` tables come with it.

### 2.2 The wheels, priced

| measurement | value |
|---|---|
| `firecrawl-anydoc` 0.2.4 | `cp310-abi3` wheels: macOS arm64 + x86_64, manylinux 2.17 aarch64 + x86_64, musllinux 1.2 |
| `pdf-inspector` 1.19.0 | `cp38-abi3` wheels: the same set plus `win_amd64` |
| bytes added to the Yi binary | **0** |
| Rust crates added to Yi's graph | **0** |
| on-disk in the venv | `anydoc` 6.9 MB + `pdf_inspector` 11 MB |
| `python -c "import anydoc"`, whole process | 30 ms |
| `python -c "import pdf_inspector"`, whole process | 10 ms |

abi3 means one prebuilt wheel per platform, no compilation, no Rust toolchain
on the user's machine, and no rebuild per Python patch release.

### 2.3 It works, on real documents

Run against four files on this machine, through the Python binding:

| input | size | markdown out | wall |
|---|---|---|---|
| `.xlsx` | 9 KB | 1.2 KB | **0.7 ms** |
| `.docx` | 36 KB | 1.1 KB | **6.0 ms** |
| `.pdf`, 6 pages | 406 KB | 7.9 KB | **29.8 ms** |
| `.pdf`, 19 pages | 106 KB | 40.6 KB | **22.4 ms** |

`pdf_inspector.detect_pdf` classified both PDFs as `text_based` with
confidence 1.00 in 3.3 ms and 5.1 ms.

Two facts from that table drive §5 and §6. The 19-page PDF produced **40.6 KB
of markdown** — roughly 10k tokens from a single `read`, so conversion output
must go through the read tool's existing windowing rather than into a tool
result whole. And `to_markdown`'s signature is
`(path, *, ocr: Literal['reject','hosted'] = 'reject', api_key=None, api_url=None)`.
Its `hosted` value is a network upload to a third-party API and is **refused
outright** by §6 — not defaulted off, refused. `reject` raises `NeedsOcrError`
locally, which is the only behaviour this design ever asks for.

## 3. The ruling: the extras array, not the crate

`crates/kernel/src/bootstrap.rs:10-27` already installs twelve packages into
the uv-managed kernel venv and already has the degradation discipline for them
("A broken skill or extra degrades to its unavailable wrapper; it must never
fail the whole venv"):

```rust
pub const DEFAULT_RLM_EXTRA_UV_ARGS: [&str; 12] = [
    "requests", "httpx", "pyyaml", "tomli", "python-dotenv", "pandas",
    "numpy", "scipy", "beautifulsoup4", "lxml", "pydantic", "tyro",
];
const DEFAULT_RLM_EXTRA_IMPORT_NAMES: [&str; 12] = [
    "requests", "httpx", "yaml", "tomli", "dotenv", "pandas", "numpy",
    "scipy", "bs4", "lxml", "pydantic", "tyro",
];
```

**Phase 1 is two entries in each array and a `12` that becomes `14`:**
`firecrawl-anydoc` / `anydoc` and `pdf-inspector` / `pdf_inspector`. Four
lines of Rust, zero binary bytes, zero Rust dependencies, one venv identity
bump. `missing_extra_packages` already installs only what is absent and
`python_imports` already probes by import name, so a machine whose image
carries them fetches nothing.

What that buys, the day it lands: every format in §2 is one `ipython` cell
away, deterministically, on every platform Yi supports — instead of depending
on whichever of `pdftotext`, `pandoc` or `soffice` happens to be on the host.
On the machine this was written on, `pdftotext` and `pandoc` are present and
`soffice` is not; `pandoc` reads `.docx`/`.epub`/`.odt` and cannot read
`.pptx`, `.xlsx`, legacy `.doc`/`.xls`, or PDF at all.

This is also, precisely, the demotion's own reasoning carried out: "the kernel
venv already reaches every converter Python has" is a claim about a venv, and
this makes it true for these formats instead of hoping.

## 4. The tool surface

Three candidates. Only one is admitted.

### 4.1 A new `doc` / `convert` tool — rejected

A tool costs a name, a description and a schema in **every** request's prompt
budget, and a knob the model has to learn to reach for. D117's ruling on the
tool surface was explicit: "A feature that needs the model to learn a knob is
out; a feature that removes a call, a re-read or a rule from the prompt is in.
No new tools." A `read` that simply works on a `.docx` removes a call. A
`convert` tool adds one the model must first remember exists.

### 4.2 `.yi/tools/` exec tool — kept for the unusual, not the design

T4 already discovers `~/.yi/tools/*` and `.yi/tools/*`, calls each with
`--schema`, and runs it with JSON on stdin (project-level tools hash-pinned
under D30). A twenty-line script wrapping `anydoc` is a working tool today
with **zero** core changes, and that is genuinely the right answer for
anything unusual — a house format, a converter with a licence Yi will not
carry, a local tool a particular machine happens to have. It is the wrong answer for the common path: it must
be installed per machine, it re-pays the prompt-budget cost of §4.1, and a
capability every user needs should not require every user to write it.

### 4.3 `read` learns the fallback — admitted, gated on §9

Today `crates/tools/src/hashline/tool.rs:385-390` is the whole story:

```rust
let raw = match std::fs::read_to_string(path) {
    Ok(raw) => raw,
    Err(error) => {
        return error_output(format!("failed to read {}: {error}", path.display()));
    }
};
```

A `.docx` dies in that one `Err` arm with `stream did not contain valid
UTF-8`. That arm is the entire seam, and the design is: **on a non-UTF-8 read,
try one conversion to a sidecar file, then read the sidecar through the code
that already exists.**

```
read("report.docx")
  └─ read_to_string → Err(InvalidData)          # the only trigger
       └─ sidecar = ~/.yi/converted/<xxh3(path, mtime, len)>.md
            ├─ hit  → read_file(sidecar)                  # 0 ms
            └─ miss → venv python, one shot, timeout + size cap
                      ├─ ok               → write sidecar, read_file(sidecar)
                      ├─ UnsupportedError → today's error, unchanged  (§4.4)
                      ├─ NeedsOcrError    → error saying why, locally (§5)
                      └─ no venv/pkg      → today's error, plus one line
                                            naming the bash command
```

The trigger is the `Err`, and nothing else — no extension table on the Rust
side, because the library sniffs content better than a table can. That is
separate from what `read` *advertises*, which is a derived list of the formats
and is the subject of §4.4.

Why the sidecar rather than returning the markdown as a tool result:

- **Windowing, caps and `find` come free.** The 40.6 KB case from §2.3 goes
  through the same `offset`/`limit`/`ranges`/`find` path as any other file, and
  the same `DETAIL_CAP`. No new read machinery, no second truncation rule.
- **Line numbers are stable and hashlines are real.** Hashline's whole contract
  is that a line identity from any earlier output still works. A markdown file
  on disk satisfies that; a synthesised string does not.
- **A second read costs nothing**, and `grep` over the sidecar works.
- **The cache key includes mtime and length**, so editing the `.docx` in Word
  invalidates it. No staleness rule to write.

Mechanics that matter:

- **Dependency direction holds.** `yi-tools` depends on `yi-types` and
  `yi-permission` only, and must not learn about `yi-kernel`. It spawns the
  venv interpreter through `crate::process::run_captured`, exactly as `BashTool`
  and `ExecTool` already spawn processes — with the existing cancel flag and
  output cap, plus a timeout and an input-size ceiling (§6). The interpreter
  path arrives through `ToolContext`; when it is absent, the fallback simply
  does not exist and the error is today's.
- **`ToolKind::Read`, unchanged.** No new permission surface, no new rule kind.
- **One direction only.** anydoc has no writer, and the plan does not pretend
  otherwise: `edit` and `write` on a source document must **refuse** and name
  the sidecar, rather than letting the model edit markdown it believes is the
  `.docx`. That refusal is the single most important line in the phase.
- **The readable set has one source of truth — the installed wheel — and it is
  both the dispatch's answer and the description's claim** (§4.4). Nothing in
  Yi retypes it.
- **`.csv` needs its format named**, which the fallback can do because it has
  the path; the library's own limitation is a CLI ergonomics problem, not ours.
- **Encrypted documents refuse** (`EncryptedError`), surfaced as-is.

### 4.4 The description names the formats, derived from the installed wheel

Two different questions hide in "how does Yi know what `read` can read", and
they get different answers:

- **What does `read` attempt?** No table. The trigger is the non-UTF-8 `Err`
  and the library answers by content markers (§4.3). A dispatch table would be
  a second copy of someone else's capability list, and worse than the library
  at its own job — anydoc detects a mislabelled `.txt` that is really an RTF,
  and no table can.
- **What does `read` *claim*?** The formats, by name, in its description —
  because a tool that can read a `.docx` and does not say so is a capability the
  model will never reach for. This is not optional and not a prompt-budget
  question: twelve short names cost almost nothing, and the alternative is a
  feature nobody uses.

Today `read`'s description is a `&'static str` literal
(`crates/tools/src/hashline/tool.rs:62`) that says "Read a file …, a directory
…, or a glob". It should say what file *types*, and the list must come from the
wheel rather than from a human retyping it.

**Where the list comes from.** `anydoc.Format` is a `typing.Literal`, so the
canonical set is machine-readable from the installed package:

```python
>>> import typing, anydoc; typing.get_args(anydoc.Format)
('doc', 'docx', 'odt', 'pdf', 'ppt', 'pptx', 'rtf', 'epub', 'xlsx', 'ods', 'odp', 'csv')
```

Twelve canonical formats, verified against this wheel. Aliases fold onto them —
`format_from_extension` maps `docm`→`docx`, `xlsb`/`xls`→`xlsx`,
`pptm`/`ppsx`/`pps`/`pot`→`ppt`/`pptx` — and returns `None`, not an exception,
for anything unsupported. So the description enumerates the twelve; the sniffer
handles the aliases without either of them being written down in Yi.

**Where it is derived, and why that cannot drift.** At the one moment the
capability can change: bootstrap. `missing_extra_packages` already spawns the
venv interpreter once per extra to probe it by import name
(`crates/kernel/src/bootstrap.rs:510-517`), so the probe that also asks for
`get_args(anydoc.Format)` is the same spawn shape, on a path that runs when the
wheel is installed or verified — not on the `--version` path, which has a 5 ms
budget.

The answer persists in the record that already exists for exactly this purpose.
`BootstrapVersion` (`crates/types/src/kernel.rs:134-146`) carries
`#[serde(flatten)] pub extra: Map<String, Value>`, written to
`.bootstrap-version` beside the venv, and **any mismatch in that file forces a
full venv rebuild (design K1)**. So `"documentFormats": ["doc", "docx", …]`
goes in `extra`, and drift is structurally impossible: upgrading the wheel
changes the venv identity, which re-runs the bootstrap, which rewrites the
list. The capability and its description change in the same operation, or
neither does.

**How the tool reads it.** `HashlineReadTool` holds an owned `String`
description built at construction — `fn description(&self) -> &str` returns
`&self.description`, no trait change — from a format list handed in by the
caller that already knows where the venv is. `yi-kernel` exposes a plain
accessor that reads `.bootstrap-version` and returns the list: a file read, no
spawn, no Python at session start. `yi-tools` still depends on nothing new
(§4.3), because it receives a `&[String]` and does not know where it came from.

**The sentence.** Concretely, with a populated list, the description gains one
clause built from it:

> … or a glob (every match). Office documents, PDFs and EPUBs — `doc` `docx`
> `odt` `pdf` `ppt` `pptx` `rtf` `epub` `xlsx` `ods` `odp` `csv` — are converted
> to read-only Markdown on read; the source is not editable through `edit`.

The names are the list, joined; the surrounding words are the only part a human
writes. It says what it can do and what it cannot, which is the pair of claims
the model needs to both reach for it and not waste a turn trying to edit a
`.docx`.

**Honest degradation.** No venv yet, or the wheels absent: the list is empty and
the description is today's sentence, verbatim. The tool claims exactly what the
machine can do, which is the whole point of deriving it. A first session on a cold machine
may therefore claim nothing and a later one claim twelve formats. That is the
correct behaviour: the clause is omitted rather than promising a capability
that is one un-run bootstrap away, and it appears the moment the capability
does.

**The gate.** `crates/runtime/tests/prompt_drift.rs` exists for precisely this
class of bug, and its own comment is the rule:

> identity.md once described grep as literal-only while the tool had been regex
> for weeks: a prompt claim about a tool is checked against the tool, not
> trusted.

So the test asserts the round trip against a **live** venv: every format name
in `read`'s description is one `typing.get_args(anydoc.Format)` returns, and
every one it returns appears in the description. A hardcoded list passes today
and fails the first time the wheel moves; the derived list cannot fail. That
test is what makes "derived" a fact rather than an intention.

One thing stays out of scope: `grep`'s NUL-byte skip is unchanged and its
description keeps saying so. Converting every binary in a tree walk to search it
is a different and much more expensive feature. The sidecar is greppable once
something has read it, which is enough.

## 5. pdf-inspector is not a second tool

`anydoc` already carries `pdf-inspector` for PDFs, so surfacing it separately
would add a tool (§4.1) to expose a classification that only matters if OCR is
on the table — and §6 refuses OCR outright. Skip it.

The one place its output earns three lines is the dead end. When anydoc raises
`NeedsOcrError`, the error text should say **why**, in the terms the exception
and `detect_pdf` already hand over — page count, classification, which pages
are image-only. That is a local classification, computed from the file, and it
turns "did not contain valid UTF-8" into "scanned PDF, 19 pages, no text
layer" — which is the difference between a dead end and a dead end the reader
understands. The message names no remedy, because §6 leaves none to name.

## 6. OCR is refused, and no call leaves the machine

**Hard constraint, not a default: nothing in this design makes a network call,
and no third-party document API is ever reached.** anydoc's hosted OCR mode
uploads the document itself to an external service — an outbound path triggered
by reading a local file, which is the shape of an exfiltration bug, on a tool
whose whole job is reading the user's files. The design already ruled on it
("Never the `ocr` feature") and this proposal hardens that from a default into
a refusal:

- The `ocr` argument is pinned to `reject` at the one call site, and
  `api_key`/`api_url` are never passed.
- `pdf_inspector.process_pdf_with_ocr` and `process_pdf_with_ocr_bytes` are
  never called, and neither is any `anydoc.Ocr` constructor.
- **There is no config key**, because a config key is a thing a session can
  flip. There is no environment variable, no tool argument, no per-repo
  setting, and no prompt path that reaches a hosted mode.
- No error message suggests hosted OCR as a workaround, so the model never
  learns the route exists and never proposes it to the user.
- The convert subprocess is a fair place to enforce this rather than trust it:
  it needs no network at all, so on platforms where that is cheap it should be
  spawned without one, and the gate test asserts the refusal directly.

A scanned PDF is therefore a **refusal with a reason** (§5), permanently. Local
OCR — a model or binary on the user's own machine, no upload — is a different
question with a different trust story, and it is not in this proposal.

The other boundary is the parser itself.

A `.docx` is a zip and a PDF is a
parser target, both attacker-shaped input:

- **Timeout and output cap** on the convert subprocess — `run_captured` already
  provides both, plus the cancel flag.
- **Input size ceiling** before the spawn, so a 2 GB PDF is an error rather
  than an OOM. anydoc raises `ResourceLimitError` for its own limits; the
  ceiling is ours.
- **A crash in the converter is an error in a subprocess**, which is a large
  part of why the subprocess is preferable to linking the parser into the
  agent's own address space, independent of the 3.42 MiB.
- The sidecar goes under `~/.yi/converted/`, named by hash, never beside the
  user's document.

## 7. Why this needs no one-out

§1.1's one-in-one-out governs **top-level features**. Against §9.2's admission
gates, this is not one:

| gate | this proposal |
|---|---|
| new crate in the graph | none — 0 Rust deps, 0 binary bytes |
| new cargo gate | none |
| new tool in the prompt | none — `read` learns a fallback |
| observes or interrupts the turn | no. Nothing touches compaction, branching, abort, steer, retry or dispose |
| core hits (§9.2's real metric) | one `Err` arm in `read_file`, plus a refusal in `edit`/`write` |
| its own delivery seam | none — it reuses `read`'s output path and `process::run_captured` |
| LOC | ~4 lines in `yi-kernel`, ~60 in `yi-tools` (budget 9,000) |

§9.2's own case study is the argument: `checkpoint` was 212 LOC against **128**
core hits and metastasised; `hashline` was 7,193 LOC against 2 and did not.
This is nearer the second. What it must never become is the first — which is
what §4.1 and §5 are protecting by refusing every new surface.

## 8. Coverage after the change

| family | formats | route |
|---|---|---|
| Word | `.doc` `.docx` `.docm` | anydoc |
| PowerPoint | `.ppt` `.pps` `.pot` `.pptx` `.pptm` `.ppsx` `.ppsm` | anydoc |
| Excel | `.xls` `.xlsx` `.xlsm` `.xlsb` | anydoc |
| OpenDocument | `.odt` `.ods` `.odp` | anydoc |
| other | `.rtf` `.epub` `.csv` | anydoc |
| PDF, text-based | `.pdf` | anydoc → pdf-inspector |
| PDF, scanned or image-only | `.pdf` | **refused**, with the local reason (§5). No OCR, no upload, ever (§6) |
| images, audio, video | — | out of scope; not a coding agent's capability |

## 9. The admission gate, and what it says today

The demotion's rebuild case is a measurement, so here it is. Across **410
sessions** in `~/.yi/sessions`:

- **0** reads failed with `did not contain valid UTF-8`.
- **28** mentions of `.pdf` in total, of which most are incidental `ls -l`
  output.
- **1** session shows the real loop: session `01a04c94`, where the user
  `@`-referenced two research PDFs and the model reached straight for
  `bash pdftotext -layout` — it never called `read` on them at all. The loop
  was closed, in one bash call, by a system binary.

**So the rebuild case for phases 2-3 is not met, and this document does not
claim it is.** One occurrence in 410 sessions, self-served in a single call,
does not buy a change to the `read` tool. That is why phase 1 is the whole of
the admitted work: it costs four lines, it makes that same bash/ipython route
deterministic and cross-platform instead of dependent on the host's poppler,
and it is the cheapest thing that could possibly help.

The trigger to build phases 2-3, stated in advance so it is not argued
retroactively: **five or more sessions in a 30-day window** in which a document
in §8's table is read, converted or worked around — countable by the same grep,
plus a `read` error signal added in phase 1's telemetry. The one thing worth
adding immediately is the count: a session-signal on the non-UTF-8 `Err` arm,
recording the extension, so the next revision of this document argues from a
number instead of a grep over filenames.

## 10. Phases

**Phase 1 — the extras and the derived list (admitted).** Two entries in each
array in `crates/kernel/src/bootstrap.rs`, `[&str; 12]` → `[&str; 14]`. The
bootstrap probe gains `typing.get_args(anydoc.Format)` and writes
`documentFormats` into `BootstrapVersion.extra`; `yi-kernel` gains the accessor
that reads it back from `.bootstrap-version`. A signal on `read`'s non-UTF-8
`Err` arm carrying the extension. Tests: the venv exercise imports both modules
and converts a committed 4 KB `.docx` fixture to markdown; the record's
`documentFormats` equals what a live `typing.get_args(anydoc.Format)` returns;
a venv missing the wheels writes no list, degrades, and does not fail the
bootstrap. Ledger: no size row needed beyond "no dependency change, 0 bytes";
venv identity bumps, so the shared-venv rebuild note applies to concurrent
suites.

**Phase 2 — `read`'s fallback and its description (gated on §9).** The `Err`
arm, the sidecar cache under `~/.yi/converted/`, the timeout and size ceiling,
the `edit`/`write` refusal that names the sidecar. `HashlineReadTool`'s
description becomes an owned `String` built from the format list, naming the
formats and saying they arrive as read-only Markdown. Tests: a `.docx` read
returns markdown; the second read hits the cache; touching the source misses it;
`edit` on the source refuses and names the sidecar; a converted read windows
through `offset`/`limit`/`find` like any file; a missing venv gives today's
error plus the bash line, **and leaves the description at today's sentence**;
and in `prompt_drift.rs`, every format the description names round-trips
through a live `anydoc.Format` and every one that returns is named.

**Phase 3 — the legible dead end (gated with phase 2).** `NeedsOcrError` turned
into an error carrying page count and classification from `detect_pdf`, both
computed locally. Test: the message names no remedy and no external service,
and a grep of the tree finds no `hosted`, no `api_key` and no
`process_pdf_with_ocr` at any call site.

**Not in any phase:** the `anydoc` Rust crate (§2.1), a `docs` cargo feature
(§2.1), a `convert` tool (§4.1), **hosted OCR or any other network call, in any
phase, behind any flag** (§6), write-back to office formats (§4.3), a
hand-written extension list anywhere in Yi (§4.4), conversion inside `grep`'s
tree walk (§4.4), and images/audio/video (§8).

## 11. Records this change owes

- `D171` in YI_DESIGN §14.6, amending the demotion row: the crate stays
  refused, with this document's +3.42 MiB against 401 KB as the reason, and the
  wheels are recorded as what replaces it. §1.1 is **not** edited — nothing is
  admitted that needs an out.
- `docs/ARCHITECTURE.md` `version:` → 0.207.0 with its `CHANGELOG.md` row.
- A size-ledger row only if a binary byte moves; phase 1 moves none, and saying
  so on the record is the point.
- The `documentFormats` key in `BootstrapVersion.extra` is part of the K1
  contract once written, so the design-note row for K1 names it: the venv record
  is what `read`'s description is derived from, and a wheel change rebuilds
  both together.
- Phase 2 carries a growth memo against the `yi-tools` budget.

## 12. As built

Seven departures from the text above, each for a reason found while building or dogfooding it:

- **The converter reaches `read` through the tool set, not `ToolContext`.** The description
  has to name the formats when the tool is constructed, before any call carries a context, so
  `builtin_tools_with` takes a `Documents` (python, recorded formats, home) and the read, edit
  and write tools share it through the hashline state. The dependency direction of §4.3 holds:
  `yi-runtime` reads the venv record and `yi-tools` never learns where the list came from.
- **The venv's directory is keyed by the extras as well as the ready check.** Two new extras
  change `extra_args`, and every session still running main's code would have seen a mismatch
  and rebuilt the shared venv, the back-and-forth the directory key was introduced to stop.
- **The clause is worded for what actually triggers it.** A UTF-8 `.csv` is read as text and
  never converted, so the description says "a file that is not UTF-8 text is converted to
  Markdown when it is one of: …", then that the copy is read-only and `edit`/`write` refuse
  the original.
- **Phase 1's error signal ships as `details.convertedFrom`** on a converted read: the gate
  it was meant to count toward was waived, and the extension on every conversion is the same
  count, already in the session record.
- **Phase 3 has no source-grep test.** The testing doctrine (`.ruler/080-testing.md`) forbids
  asserting on source text. The refusal is proven behaviourally instead: a scanned PDF from a
  real producer (Quartz, via `sips`) comes back as the local refusal and leaves no copy, and
  the only `ocr` value the converter can pass is the literal `"reject"`.
- **RTF is offered by its marker, not only by the UTF-8 failure.** RTF is 7-bit text, so the
  non-UTF-8 trigger never fired on it and the model got raw `{\rtf1…` markup (an 820,000-line
  file on this machine). A file whose text starts `{\rtf` now goes to the converter too, and
  falls back to the raw text if nothing converts. This is a content marker, not an extension.
- **A partly scanned PDF converts its text pages.** anydoc's `reject` refuses a whole PDF if
  one page lacks a text layer, and a real 11-page PDF with one such page came back as a
  refusal. `pdf_inspector.process_pdf`, the local non-OCR path, now supplies the text pages
  under a first line naming the pages left out. Only a PDF with no text page is refused.

Dogfood, 2026-09-10: 141 documents sampled from `~/Downloads`, `~/Desktop` and
`~/Development`, covering every extension in §8, were read through the built tools. 118
converted, cold in a median of about 55-110 ms (1.1 s at most) and 0 ms from the copy. 18
were refused with a stated reason, and 5 were read as text because they were text (two UTF-8
csv files, a web2py template named `.pdf`, a spreadsheet saved as text, and a Word lock file
`~$…docx`). `find=`, windows, and the `edit`/`write` refusals behaved on copies of real files.

### Round two, 2026-09-10: the exhaustive review

After the first dogfood the owner asked for every open issue in the tools, exhaustively;
47 came back, and the same day all but four were fixed on the same branch. What changed,
by the review's numbering:

- **Bugs 1-9.** Each conversion attempt stages under its own name (a 3-thread probe had
  40 of 60 parallel first reads fail on one shared name). The two wheels are optional for a
  `YI_KERNEL_PYTHON`. Copies are read-only, the Markdown's own hash is in the copy's name, and
  a mismatch converts again; `edit` and `write` refuse the copy dir. Refusals read the bytes,
  not the cache: any existing binary document or NUL-bearing file is refused, an RTF or a
  decoded text file is not. A glob charges the Markdown to its budget and shows a document it
  cannot fit as `(<kind> converted to N lines of Markdown)`. The cache key hashes the bytes,
  so no timestamp tick can serve a stale copy. Staging files older than an hour are swept.
  File names go to the converter as `OsStr`.
- **Files 10-16.** The ceiling is 256 MiB, 1 GiB with `pages=`. A converter limit names
  pandas in ipython for a spreadsheet. Latin-1 and UTF-16 text is decoded and shown. A NUL-
  bearing non-document (a Word lock file) is refused as binary. The RTF marker is read past a
  byte-order mark and leading whitespace. The no-text refusal no longer calls a logo "scanned".
  An image is named as one, with the `attach_image` skill as the way to show it.
- **Output 17-26.** No clipping on a copy. Spreadsheets lose empty rows and cell padding and
  gain a `[sheets: name RxC, …]` line. A capped read of Markdown ends with a heading outline;
  `find=` on Markdown returns the heading section (the resolver also serves `PUT N*` on `.md`
  files). PDFs carry `[page N]` markers. The header keeps the original's path, so the note is
  one short line and no path outside the working tree reaches the model. csv is shown as text.
  Converter errors are plain words, not exception names.
- **Speed 27-31.** A file whose head carries none of the four container markers never spawns
  Python. Copies older than 30 days or past 256 MiB in total are evicted on a miss. Old venvs
  are not pruned (29): another session on another commit may be using one.
- **Discoverability 32-38.** identity.md names the capability; the description lists the
  aliases the wheel confirms (`docx (docm)`, `xlsx (xls, xlsm, xlsb)`), the limits and the
  read-only rule, and is asked again on every request so a venv built mid-session shows up;
  `bash` names `read` when a command reaches for `pdftotext`, `pandoc` or `textutil`; the
  not-built message says the venv builds at session start. No live model run was made (32).
- **Ergonomics 39-41.** `pages="3-5,9"` on a PDF. A multi-file edit that names a document
  still fails whole (40): a partial patch is worse than a refused one. The header path fixes
  41: re-reading the copy by the path the model saw was an ask, since it sits outside the tree.
- **Security 42-44.** The cache dir is 0700 and its copies expire; the sandbox's one writable
  root is the cache dir; on Linux the converter runs in a user and network namespace when
  `unshare -Urn` is allowed (a probe, once per process), else as before.
- **Tests 45-47.** Concurrency, ceiling, timeout, cancellation, Unicode and (Linux) non-UTF-8
  names, tamper, unread-document writes, decoding, glob budget, sheets, outline and section
  each have a test, and each was seen red under a mutation. Reads carry `converted: {from,
  cache, ms}` and `sourceBytes`, and a refusal carries its reason in `details`. The probe's
  hash rides in the venv record, so a changed probe re-asks the wheel instead of serving an
  old list (the drift test caught exactly that on this branch).

Not done: upstream extraction quirks (22), stray image lines (23), venv pruning (29), the
live eval (32).

Dogfood, round two, same 141 files: 118 converted (cold median about 110 ms; 5.3 s at most,
on a 239 MB Latin-1 csv now decoded rather than refused), 17 refused with a stated reason,
2 decoded, 4 read as the text they were. Thirty PDFs carry page markers; fifteen sheets carry
a summary line; the 127 MB portfolio now converts; the Word lock file is named as binary.

### Round three, 2026-09-10: playing Yi

The owner asked for a dogfood "as if you were Yi agent": 38 tool calls in one live session
(`read`, `edit`, `write`, `grep`, `bash`, the `ipython` kernel) on the math-eval-grader and
gsea-proteomics terminal-bench tasks and a coursework folder, each call chosen from the output
of the last. The refusals held; three things gave wrong answers or dead ends:

- **PDF table detection misattributes cells.** The schedule PDF paired CPSC 380 with CPSC 370's
  slot, and an answer key split `(5, -10)` across cells (a naive parse of `read`'s text scored
  47/48). The library's own Markdown makes the same error; its reading-order text does not. The
  owner's call: plain text for PDF tables, with a hint that the conversion is lossy. A page with
  a detected table now reads full width in reading order (a row stays whole), a columned page
  reads left column then right (split at the gutter the fewest text runs cross; a "table" of two
  cells on a columned page is its columns), both under `[page N: … flattened to plain text in
  reading order; cell and column boundaries are lost]`, with the page's headings restored.
- **The spreadsheet hint named a route the venv could not take**: pandas without `openpyxl`.
  `openpyxl` joins the extras, and a test runs `pandas.read_excel` in the venv.
- **`grep` could not see inside a document** and returned 28 KB of RTF markup instead. It now
  searches a document through the Markdown `read` shows (a cached copy always, up to 20 new
  conversions per call, never in `replace` mode) and names what it could not search.

And the costs: running headers are left out once (a line at the edge of 40% of pages and never
inside one); decks read slide by slide (each slide converted alone from a re-zipped deck, since
anydoc keeps no slide boundary); a one-page PDF carries no marker; sheets lose empty columns,
take a real header row, name themselves from the workbook, and point a sheet past 500 rows at
pandas; a copy's first look is 12 KB with its outline first; the outline keeps the shallowest
levels that fit, sampled across the text, repeated titles once; `find=` folds typographic quotes
and dashes; a copy adds no code refs (they grepped the raw source); a glob converts only while
it has room and names a refusal; source hashes are kept per process for files settled two
seconds; the kernel mutes pip's version notice; the ipython description names the libraries.

Replayed on the same files: the answer keys parse 48/48 from `read`'s text alone, the schedule
answer is right, the proteomics workbook opens in pandas on the first try, `grep` finds "Faraday
pail" in the lab report and names the scanned worksheet it could not search, the epub's outline
names chapters 2 to 11, the book's first look is 16 KB instead of 57 KB, and a document `find`
takes 26 ms instead of 1.3 s. Still upstream: equations fragment, and a column the library does
not detect still interleaves.

