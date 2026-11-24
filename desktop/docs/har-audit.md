# HAR audit

Written by task HA at the end of the build: every crate read against the checklist of each HAR skill (`har`, `har-api`, `har-verify`, `har-supply`, `har-layout`, `har-concurrent`, `har-async`, `har-io`, `har-threat`, `har-hot-path`, `har-unsafe`), nothing taken on trust from the per-task gates. Line numbers are as of this commit.

## Verdict

**Proven** (a check that fails when the property breaks, seen failing):

- No `unwrap`, `expect`, `panic!`, `todo!`, `unreachable!`, `unimplemented!`, byte-index string slice or `String::truncate` outside tests: clippy denies each in every crate (`Cargo.toml` `[workspace.lints.clippy]`, `#![deny(clippy::string_slice)]` in every root, `clippy.toml`), and `tools/gate/har.py` backs it; its `--selfcheck` fires on 26 bad samples, two of them added here and two (print macros) by task FX2.
- `unsafe` exists only in `crates/mac`, every block and `unsafe impl` has a `// SAFETY:` note, and the crate denies undocumented and multi-operation blocks: `har.py` checks all three (the last two are new here).
- Only `yd-sources` starts a process (clippy `disallowed_methods`); every spawn is in the spawn table below.
- A folder the app looks at no longer runs its own `core.fsmonitor` or textconv command (`git::tests::reading_a_folder_never_runs_a_command_its_config_names`, red against each control removed in turn).
- The caps a hostile peer pushes on (wire frame, language-server body, browser picture, table rows, a design colour, a triage file that does not read): six tests in five crates each fail when the cap is broken (Mutants).
- Quitting waits up to three seconds for the source threads (`start::tests::quitting_waits_for_sources_up_to_the_grace`); a triage save never writes through a link at its temporary name (`a_save_never_writes_through_a_link_at_its_temporary_name`).

**Checked, not proven** (read line by line, no test that would fail): the 22 `unsafe` sites against AppKit's contracts; every thread's stop path and every lock's hold; the arithmetic, index and cast sites clippy's restriction lints list (none can panic); every outside input's bound; that the panes' draw paths allocate per frame only as far as Rune makes them (no profile was taken).

**Remains**: children that inherit the app's whole environment, git filters a folder's own config names, `cargo vet` and unpinned `cargo-deny` and `cargo-audit`, and no fuzz, property or bounded-proof rung for the parsers. Each is under Findings with its exact change. The fix list, `overflow-checks`, the panic hook, the stand-in page and the path pins were closed by task FX2.

## How to run it again

| What | Command |
|---|---|
| The inventories (allow, unsafe, spawn, clock, random, thread, lock, write) | `python3 tools/gate/inventory.py [kind ...]` |
| Index, slice, division, cast and size arithmetic | `cargo clippy --workspace --lib --bins --message-format=short -- -W clippy::indexing_slicing -W clippy::arithmetic_side_effects -W clippy::cast_possible_truncation -W clippy::cast_sign_loss -W clippy::cast_possible_wrap -W clippy::integer_division -W clippy::as_conversions` |
| The gate and its own refute pass | `python3 tools/gate/har.py --selfcheck && python3 tools/gate/har.py` |
| Outside input | `rg -n 'const (MAX\|CAP\|LIMIT\|CHUNK\|BOUND\|QUEUE\|EVENTS\|SCROLLBACK\|BATCH)[A-Z_]*: ' crates -g '!**/tests*'` plus the reads below |
| Supply | `just supply` (cargo deny, cargo audit) |

## Findings

Severity: **High** a reachable unsafety or code run on a hostile input; **Medium** a bound, a stop or a durability gap with a plausible trigger; **Low** a gap with no plausible trigger here, or a rule's letter.

| # | Skill | Sev | Where | Finding | State |
|---|---|---|---|---|---|
| 1 | har-threat (E) | High | `crates/sources/src/git.rs:57`, `:166` | The git source runs `git status` and `git diff` in a thread's folder; a folder's own `.git/config` `core.fsmonitor` and a `diff.<driver>.textconv` named by its `.gitattributes` ran as the user the moment the folder showed (probed with real git: the marker file appeared). | Fixed: `-c core.fsmonitor=false` on every run, `--no-textconv` on the diff. |
| 2 | har-unsafe | High | `crates/mac/src/status_item.rs:67` | `pick:` called the handler through `&self` with nothing holding the target: a handler that drops its `StatusItem` frees the target and its boxed handler while they run. The app's handler only posts through `AppHandle::update`, but `StatusItem::new` is a safe fn and takes any closure. | Fixed: `self.retain()` for the call. Untestable in `cargo test`: status items are refused off the main thread. |
| 3 | har-unsafe, har-layout | Medium | `crates/mac/src/status_item.rs:71` | `eprintln!` inside an Objective-C action frame panics when stderr is closed, and a panic there aborts. | Fixed: `let _ = writeln!(stderr)`. |
| 4 | har-async, har-concurrent | Medium | `crates/app/src/start.rs` `quit` | Source threads were detached at quit: a shell's hang-up, a browser's stop and a language server's shutdown were not waited for. | Fixed (blocker class, the struct field and `spawn`'s return are 9 lines outside `quit`): each source's `JoinHandle` is kept (finished ones pruned on each start) and `quit` waits up to `SOURCES_GRACE` (3 s, from before the client's own 2 s close) for them. |
| 5 | har-io | Medium | `crates/core/src/findings/triage.rs:360` | The save created its temporary with `File::create`, which writes through a link planted at `findings.json.tmp`, and left the temporary behind on failure. | Fixed: remove what is there, `create_new`, remove the temporary on any failure; folder sync was already there. |
| 6 | har-threat (D) | Low | `crates/sources/src/server.rs:38` | The daemon's `invocation` file (another program's) was read whole, then cut to 64 KiB. | Fixed: `File::take(64 KiB)` before allocation. |
| 7 | har-async | Low | `crates/sources/src/dap/client.rs:275` | The debugger's registry entry relayed its bounded ask channel into an unbounded one, so a stuck debugger grew a queue instead of refusing. | Fixed: `sync_channel(QUEUE)`. |
| 8 | har | Low | `crates/sources/src/docker.rs:93`, `:597` | `Instant::now() + d` panics on overflow; `poll` is a `pub` parameter. | Fixed: `checked_add`, as every other deadline here. |
| 9 | har-threat (S) | Low | `crates/sources/src/server.rs:65`, `crates/sources/src/pty.rs:222` | With no `HOME`, `yi` fell back to the relative `.local/bin/yi`, looked up from the app's working folder; the allow's reason ("by absolute path") did not hold. | Fixed: a relative path is refused before the spawn. |
| 10 | har-layout | Low | `crates/core/src/trace.rs`, `crates/sources/src/lsp/client.rs` | Both at the 800-line limit. | Split: the trace tests to `trace/tests.rs` (the conventions' seam for long tests; `owners.json` gives TR the folder), and the questions asked of a server (`Ask`, `Spot`) to `lsp/client/ask.rs`. Now 633 and 725 lines. |
| 11 | har-verify (gate) | Low | `tools/gate/har.py` | The mac root's `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` denies, and a `// SAFETY:` above an `unsafe impl`, were not checked. | Fixed: both checked, each with a bad sample that the old checks pass (seen: `did not fire: mac root lets an unsafe block go undocumented`, `did not fire: unsafe impl without SAFETY`). |
| 12 | har-io | Medium | `crates/ui/src/theme/files.rs:488-499`, `crates/panes/src/script_pane/pane/library.rs:655-662` | Theme and plugin writes: `File::create` at a fixed temporary name (follows a planted link), no folder `sync_all` after the rename, the temporary left on failure. Not a tear (the file is whole either way); a crash can lose the rename. | Fixed: both go through `triage::replace`, the save's own shape (`a_theme_write_never_goes_through_a_link_at_its_temporary_name`, `a_plugin_write_never_goes_through_a_link_at_its_temporary_name`). |
| 13 | har-layout | Low | `crates/app/src/start.rs:129`, `:141`, `:157`, `crates/app/src/windows/hub.rs:504`, `crates/panes/src/settings/pane/bind.rs:120` | `eprintln!` panics when stderr is gone (an app started with it closed). | Fixed: `let _ = writeln!(std::io::stderr(), …)`; `har.py` refuses a print macro under `crates/` outside tests (selfcheck samples `eprintln`, `println`, clean `writeln to stderr`). |
| 14 | har-threat (I) | Medium | `dap/client.rs:155`, `lsp/client/server.rs:162`, `git.rs:53`, `docker.rs:83`, `launchd.rs:69`, `server.rs:71` | Every child but curl (`fetch.rs:160`, `env_clear`) inherits the app's whole environment; a language server reads a hostile repository and sees every key the app was started with. The shell (`pty.rs`) inherits on purpose. | Left, in `needs`: `env_clear` plus an allowlist (`PATH`, `HOME`, `LANG`, `TMPDIR`, and for rust-analyzer `CARGO_HOME`, `RUSTUP_HOME`) needs the owner's list. |
| 15 | har-threat (E) | Medium | `crates/sources/src/git.rs:53` | A `filter.<driver>.clean` or `process` named by a folder's own `.git/config` and `.gitattributes` still runs on `git status` of a changed file; git has no switch that turns every filter off. Needs the folder's config as well as its files, so a clone cannot carry it; a copied or unpacked folder can. | Left, in `needs`. |
| 16 | har-threat (E) | Low | `crates/sources/src/lsp/client.rs:61` | rust-analyzer starts with build scripts, proc-macros and `cargo check` off (already an owner decision to revisit), but its `cargo metadata` still reads a repository's `.cargo/config.toml`, which can name a `rustc` to run. | Left: the per-folder trust setting the code already names. |
| 17 | har-supply, har-layout | Medium | `Cargo.toml` | `[profile.release]` (Rune's) has no `overflow-checks = true` and there is no dist profile; no `cargo vet` (`supply-chain/`); `cargo-deny` and `cargo-audit` are not pinned; Rune and Yi are path dependencies on sibling checkouts, so a build is reproducible only with those checkouts at a known commit; `main` installs no panic hook. | Fixed in part: the `dist` profile (release with `overflow-checks = true`) is what `just package` ships; `main` installs a hook that writes one line to stderr and the app's log, then runs the default (`a_panic_is_one_line_in_the_apps_own_log`); Rune is pinned by git revision and Yi's two crates are in the same repository (`docs/deps.md`). Left, in `needs`: `cargo vet`, pinned `cargo-deny` and `cargo-audit`. |
| 18 | har-concurrent | Low | `crates/sources/src/pty.rs:254` | The reaper holds the child's lock across `child.wait()`, so a `stop` that wants to signal waits behind it. Tried to reproduce (a shell that closes every descriptor and `exec sleep 30`): on macOS the master gets no end of file while the session's controlling terminal is held, so the reaper reaches `wait` only after the group is gone. | Left: not reachable as seen; no test kept, since it never went red. |
| 19 | har-concurrent | Low | `crates/sources/src/pty.rs:52`, `crates/ui/src/term/view/state.rs:141` | The terminal parser's mutex is shared by the reader thread and the UI's draw. Held per 4 KiB slice of parsing, so a draw waits at most one slice. | Left: bounded by design (`SLICE`). |
| 20 | har-async | Low | `docker.rs:109`, `git.rs:85`, `launchd.rs:93`, `dap/client.rs:268` | After both pipes close, `child.wait()` has no deadline: a child that closes its output and keeps running holds the source's thread. Each is a system binary that exits when its output ends. | Left. |
| 21 | har-threat (S) | Low | `lsp/client/server.rs:136`, `dap/client.rs:236` | A language server and `gdb` are found on `PATH` when first asked for, not once at start. A Dock launch's `PATH` is the system's. | Left. |
| 22 | har-api | Low | `crates/sources/src/cdp.rs:385` | `cdp::Scripted`, a stand-in page, is public non-test code because tests in `yd-sources` and `yi-desktop` drive it. It should sit behind a `test-support` feature of `yd-sources` that `yi-desktop` enables only under `[dev-dependencies]`. | Fixed: `Element` and `Scripted` are `cfg(any(test, feature = "test-support"))`; `yd-sources`' dist library held 14 `Scripted` symbols, now 0. The design's feature list does not have the row yet. |
| 23 | har-layout | Low | `crates/core/src/command/tools/remote.rs:150` | `yd-core` builds the recorded desktop's pixels, where the architecture says it holds none. It is a constant of the record with no I/O, and both the source and the pane's scenes need it; `yd-panes` cannot reach `yd-sources`. | Recorded as an exception in `docs/architecture.md` (Machines). |
| 24 | har-verify (gate) | Low | `tools/gate/har.py` `strip_tests` | The test-module stripper counts braces inside string and character literals (`'}'` in `start.rs`'s tests), so its own panic scan of code after such a module is off by a module. Clippy's denies read `cfg(test)` exactly, so the rule is still enforced. | Left. |
| 25 | har-verify | Medium | the parsers | No property, fuzz or bounded-proof rung exists for the wire frame reader, the LSP and DAP header readers, the address decoder or the theme file reader; `proptest`, `cargo-fuzz` and `kani` are not admitted or installed. | Left, in `needs`. |
| 26 | har-io | Low | Rune `prefs.rs` | The settings file goes through Rune's `Prefs`: read whole and renamed without a folder sync. Rune's code, outside this repository. | Left (noted for Rune). |
| 27 | har-threat (D) | Medium | `crates/sources/src/git.rs:305` | An untracked file the changes pane shows was read whole on every git poll, then cut to 2 MiB: a dump or a disk image in the thread's folder was loaded in full each time. | Fixed: `File::take(2 MiB + 1)` before allocation (`a_new_file_past_the_text_cap_comes_back_cut`). |

**Counts.** Found 27: High 2 (fixed 2), Medium 9 (fixed 5, in part 1, left 3), Low 16 (fixed 8, left 8). By skill (a finding under two skills counts for the first): har-threat 8, har-unsafe 2, har-async 3, har-io 3, har-layout 3, har-verify 3, har-concurrent 2, har 1, har-supply 1, har-api 1; har-hot-path 0 (nothing measured, see below).

## Fix list (another agent's files this wave; both done by task FX2)

**A. Theme and plugin writes** (`crates/ui/src/theme/files.rs:488`, `crates/panes/src/script_pane/pane/library.rs:655`): write as `yd_core::findings::triage::save` now does:

```rust
let copy = path.with_extension("json.tmp");
let _ = std::fs::remove_file(&copy);
let written = File::options().write(true).create_new(true).open(&copy)
    .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.sync_all()))
    .and_then(|()| std::fs::rename(&copy, path))
    .and_then(|()| match path.parent() { Some(dir) => File::open(dir)?.sync_all(), None => Ok(()) });
if written.is_err() { let _ = std::fs::remove_file(&copy); }
```

**B. Five `eprintln!`** (`crates/app/src/start.rs:129`, `:141`, `:157`, `crates/app/src/windows/hub.rs:504`, `crates/panes/src/settings/pane/bind.rs:120`): `let _ = writeln!(std::io::stderr(), "…");` with `use std::io::Write`.

## Per skill

### har

Checked: the panic table (every row), Result discipline, newtypes, exhaustive matches, integer rules. How: clippy's denies and the restriction-lint run above (244 sites, triaged below); `rg '_ =>'` over enums the crate owns was not run whole: the panes' matches over `SourceCmd` and `FeedEvent` were read (`pty.rs:166` lists every variant). Passed: no panicking call, no unchecked size arithmetic on input, no `as` on a wire value (float to integer `as` saturates and every one from outside is clamped first: `cdp.rs:124`, `settings/store/value.rs:23`). Findings: 8.

### har-api

Checked: error types at boundaries (`thiserror` in each crate's public errors), `Debug` derives, `#[must_use]` on results lost on drop (`Client::close` returns `Result`), visibility of test doubles. Findings: 22.

### har-verify

Checked: every rung against what exists. Rungs 0 and 1 (clippy, tests against recorded fixtures) are the gate; snapshot goldens are rewritten only with `YD_BLESS=1`; no rung 3 to 8. Mutation adequacy by hand below. Findings: 11, 24, 25.

### har-supply

Checked: the four questions. `cargo deny` (licences, duplicates outside Rune's skipped tree, sources: one pinned git revision) and `cargo audit` run in `just supply` (exit 0; `cargo audit` allows three unmaintained warnings, `zbus_names`, `zvariant`, `zvariant_derive`, RUSTSEC-2026-0370 to 0372, in Rune's Linux-only tree); no `cargo vet`; workspace lints opted into by every member (`har.py`); toolchain pinned (`rust-toolchain.toml` 1.99.1, `rust-version` set); `Cargo.lock` committed; `cargo tree -e build` not reviewed per entry (Rune's tree). Findings: 17.

### har-layout

Checked: flat `crates/` with folder = crate (`yd-` prefix), the architecture document (codemap, invariants, trust boundaries all present), naming (`util`, `helpers`, `manager`: none), panic policy, stdout and stderr. Findings: 3, 10, 13, 17, 23.

### har-concurrent

Checked: every thread and lock (tables below), poison policy (every `lock()` is `unwrap_or_else(PoisonError::into_inner)` or `if let Ok`), atomics (`AtomicBool` stop flag `Relaxed`, publishing nothing; two id counters `fetch_add(Relaxed)` that would wrap after 2³² shells or 2⁶⁴ pages), channels (bounded wherever a peer can fill them; the unbounded ones carry one message each). Findings: 4, 18, 19.

### har-async

No async runtime; the subprocess, pipe and EOF rules apply to the threads. Checked per spawn: one drainer per captured pipe, stdin dropped before waiting, group kill and reap, a deadline on each wait a peer holds. Findings: 4, 7, 20.

### har-io

Checked: every write (table below), every clock read, every random read, every text cut (`chars().take`, `is_char_boundary` walks; `string_slice` denied). Wall clock read once (`server.rs:95`, the wire's Unix milliseconds, `duration_since` defaults to zero); every deadline is `Instant`. No random value is security-bearing: the only randomness is `HashMap`'s `RandomState`. `DefaultHasher` (fixed keys) is used only for in-process change stamps. Findings: 5, 12, 26.

### har-threat

Boundaries as `docs/architecture.md` lists them, plus the folder a thread names (git, the language server). STRIDE per boundary:

| Boundary | S | T | R | I | D | E |
|---|---|---|---|---|---|---|
| Socket to `yi serve` | anything that binds `~/.yi/daemon.sock` answers; the socket is the user's own (accepted) | frames checked by sequence in the reducer | none kept beyond `Wire.failures` (accepted: a client) | the app sends nothing secret | 32 MiB frame cap, skipped not fatal; every table capped | an ask is answered only by a press |
| Child processes | absolute paths, found once (21) | outputs parsed typed | — | environment inherited (14) | output and time caps per spawn | a folder's config runs code (1, 15, 16) |
| Web content, design answers | — | typed decode; a wrong shape refuses the whole answer | — | — | picture cap, 64 questions, 64 elements | values reach the page only as JSON data |
| Theme, settings, triage, plugin files | — | typed, refused by name | — | — | 256 KiB, 4 MiB, 256 KiB | a link at a temporary name (5, 12) |
| Pasted addresses | — | `%XX` must decode | — | — | 2,048 bytes | `yi://tools`, `yi://skills` refused |

**A design value reaches the page only as data**: `cdp::tests::an_expression_carries_typed_words_as_data_never_as_script` (`crates/sources/src/cdp.rs:580`: words that try to close the script, `"});alert(1);({"`, read back whole as the argument's value, and a value carrying a `;` is refused), `browser::design::a_value_that_is_not_one_is_refused_with_a_note_and_changes_nothing` (`crates/sources/src/browser/design.rs:206`), `command::tools::design::tests::a_typed_value_is_kept_only_when_it_is_a_value_of_its_property` (`crates/core/src/command/tools.rs:290`), `design::pane::tests::a_bad_colour_is_refused_in_place_and_a_good_one_is_asked_of_the_page` (`crates/panes/src/design/pane/tests.rs:181`, killed a mutant below), and what the page answers: `cdp::tests::an_over_long_string_from_the_page_is_cut_at_a_character_boundary`, `a_number_from_the_page_is_held_in_bounds_and_a_wrong_shape_is_refused`. The script sets values only through `style.setProperty` and `textContent` (`cdp.rs:49-59`), never `innerHTML`.

Findings: 1, 6, 9, 14, 15, 16, 21.

### har-hot-path

Checked by read: the list engine's fast path (versioned keys, rows on demand), bounded caches (`MAX_FETCHED`, scrollback 10,000), the PTY reader's reused 64 KiB buffer. Not measured: no profile or benchmark of a frame was taken, so no claim is made. Findings: none.

### har-unsafe

Every site read against AppKit's contract (table below), the module read whole (step 5 of the audit list), `mem::forget` of `StatusItem` considered (leaks the item in the bar; nothing unsound). Miri cannot run FFI. Findings: 2, 3.

## Inventories

### `#[allow]` and `#[expect]`: 21 (11 outside tests, no `#[expect]`)

`python3 tools/gate/inventory.py allow`

| Site | Lint | Reason | Holds |
|---|---|---|---|
| `crates/mac/src/lib.rs:8` | `unsafe_code` | the one module that calls into Objective-C | yes: one module, `deny` at the root |
| `crates/sources/src/dap/client.rs:154` | `disallowed_methods` | the debug adapter spawn | yes |
| `crates/sources/src/dap/client.rs:248` | `disallowed_methods` | the xcrun lookup | yes: `/usr/bin/xcrun` |
| `crates/sources/src/docker.rs:82` | `disallowed_methods` | the docker spawn | yes: fixed absolute list |
| `crates/sources/src/fetch.rs:157` | `disallowed_methods` | the curl spawn | yes: fixed absolute list |
| `crates/sources/src/git.rs:52` | `disallowed_methods` | the git spawn | yes: fixed absolute list |
| `crates/sources/src/launchd.rs:68` | `disallowed_methods` | the launchctl spawn | yes: three named binaries by absolute path |
| `crates/sources/src/lsp/client/server.rs:151` | `disallowed_methods` | by absolute path with an argument list | yes: `which` returns absolute paths only |
| `crates/sources/src/pty.rs:198` | `disallowed_methods` | the shell through Yi's PTY shim by absolute path | now yes (9) |
| `crates/sources/src/server.rs:58` | `disallowed_methods` | yi by absolute path with an argument list | now yes (9) |
| `crates/sources/src/server.rs:90` | `disallowed_methods` | the one wall clock the wire client is given | yes: the only `SystemTime::now` |
| test files (10) | `dead_code` ×6 on shared `support` modules, `disallowed_methods` ×4 to build a scratch repository or probe | as written | yes |

### `unsafe`: 22, all in `crates/mac/src/status_item.rs`

`python3 tools/gate/inventory.py unsafe`. Every operation runs on the main thread: `StatusItem` holds a `MainThreadMarker` (not `Send`) and `new` refuses elsewhere.

| Line | Operation | Contract | Holds |
|---|---|---|---|
| 47 | `unsafe impl ClassType for Target` | NSObject subclass, `MainThreadOnly`, ivars dropped by objc2 | yes |
| 58 | `unsafe impl Target` (`pick:`) | an action method takes the sender, returns nothing | yes; the sender is an `NSMenuItem` since only `row` sets this target |
| 63 | `sender.tag()` | a live item, main thread | yes |
| 94 | `msg_send_id![super(alloc), init]` | init once on a fresh alloc | yes |
| 96, 186 | `NSStatusBar::systemStatusBar()` | main thread | yes |
| 98 | `statusItemWithLength` | main thread, AppKit's constant | yes |
| 101 | `setMenu` | the item retains the menu | yes |
| 113, 126 | `button(mtm)` | main thread | yes |
| 115 | `setTitle` | the button copies the string | yes |
| 128 | `title()` | returns a retained copy | yes |
| 134, 145 | `numberOfItems` | main thread | yes |
| 138 | `performActionForItemAtIndex` | index below the count (checked) | yes; it calls `pick:` at once, now with the target retained (2) |
| 148 | `itemAtIndex` | index below the count just read | yes |
| 153 | `setTarget(None)` | AppKit does not retain a target | yes |
| 156 | `removeAllItems` | every row cut loose first | yes |
| 166 | `setTarget(Some)` | the target outlives every row that names it | yes: `clear` runs before the target is dropped, in `set` and `Drop` |
| 168 | `setTag` | a plain integer | yes |
| 178 | `initWithTitle_action_keyEquivalent` | alloc and init with live strings | yes |
| 188 | `removeStatusItem` | the item this bar made | yes |

### Process spawns: 9

`python3 tools/gate/inventory.py spawn`

| Site | Program and arguments | Working folder | Shutdown | Inherits |
|---|---|---|---|---|
| `dap/client.rs:155` | the adapter found by `locate` (xcrun, `PATH`); fixed arguments | the app's | `Link` drop kills the group and reaps | environment; stdin and stdout piped and drained, stderr null; own group |
| `dap/client.rs:249` | `/usr/bin/xcrun -f <name>` | the app's | killed past 5 s, else waited (20) | environment; own group |
| `docker.rs:83` | first of a fixed list; arguments built from typed asks (container names checked) | the app's | group killed past `DEADLINE` or 8 MiB, else waited (20) | environment, `LC_ALL=C`; own group |
| `fetch.rs:158` | system curl; a plugin's URL as one argument after checks | the app's | group killed past its 5 s limit, always stopped | nothing (`env_clear`); own group |
| `git.rs:53` | first of a fixed list; `-C <thread folder>`, fixed verbs, a hunk on stdin | via `-C` | group killed past `TIMEOUT` or 32 MiB, else waited (20) | environment, `LC_ALL=C`, `GIT_OPTIONAL_LOCKS=0`, `core.fsmonitor=false` (1) |
| `launchd.rs:69` | `/bin/launchctl`, `/usr/bin/plutil`, `/usr/bin/id`; labels checked to plain characters | the app's | killed past `TIMEOUT` or 1 MiB, else waited (20) | environment |
| `lsp/client/server.rs:162` | the server `which` finds; the table's arguments | the thread's folder | `shutdown` and `exit`, then the group killed past 2 s | environment; own group |
| `pty.rs:226` | `yi __pty <shell> [args]`; `yi` absolute (9) | the thread's folder | hang-up to the group, kill after 2 s; joined at quit (4) | environment on purpose (the user's shell), `TERM`, `LANG` if unset; the PTY as stdio; `__pty` makes it a session leader |
| `server.rs:71` | `yi serve --socket <path>` plus the recorded flags (64 KiB cap, 6) | the app's | none: the daemon outlives the app by design; a reaper thread waits on it | environment; stdio null; own group |

### Clock reads: 35; random reads: 15

`python3 tools/gate/inventory.py clock random`

| Where | Clock | For |
|---|---|---|
| `server.rs:95` | `SystemTime` | the wire's Unix milliseconds; a backwards step clamps to zero |
| `core/wire/client.rs` (9), `lsp/client.rs` (2), `lsp/client/server.rs` (5), `dap/client/session.rs` (4), `docker.rs` (4), `fetch.rs` (2), `browser.rs` (4), `app/start.rs` (3), `script_pane/pane/library.rs` (1) | `Instant` | deadlines, backoff, handshake latency, a plugin run's watchdog, quit's grace |

Random: the 15 sites are `HashMap` and `HashSet` construction (`RandomState`, seeded by the OS). No key, nonce or token is generated anywhere; nothing here needs a CSPRNG.

### Index, slice, division, cast and size arithmetic: 244 sites from clippy's restriction lints

| Kind | Count | Verdict |
|---|---|---|
| `indexing_slicing` | 6 | `serde_json::Value` index-assign on object literals (`core/wire/request.rs:90-133`) and a constant array (`panes/code_view/pane/scenes.rs:63`): cannot panic |
| `arithmetic_side_effects` | 39 | constants of the record (`core/command/tools/remote.rs`), counters bounded by an input count, scene sizes; `docker.rs` deadlines fixed (8). None on a value from outside without a bound |
| `integer_division` | 64 | every divisor a nonzero constant, `max(1)` or guarded (`trace.rs:244` checks `took > 0`) |
| casts | 135 | widening `as`, float to integer (saturating in Rust, clamped where the value comes from outside: `cdp.rs:124`, `settings/store/value.rs:23`, `tokens.rs:111` filters non-finite), `as usize` of `u32` on 64-bit |

### Outside input

| Input | Read at | Bound | A hostile input |
|---|---|---|---|
| Wire frames | `core/wire/frame.rs:369` | 32 MiB a frame; 100,000 messages, 10,000 threads, 64 asks, 1,024 agents | over the cap: skipped and counted, the stream resynchronises (mutant killed) |
| Language-server messages | `sources/lsp/protocol.rs:36` | 16 headers of 4 KiB, 32 MiB body; 500 references, 64 files | over the cap: the server ends with an error (mutant killed) |
| Debug-adapter messages | `sources/dap/protocol.rs:29` | 1 KiB header, 16 MiB body; 64 frames, 1,000 variables, 500 output rows | ends the session |
| Web pictures | `ui/screen/surface.rs:120` | 8,192 a side, 8,192 × 4,320 pixels, refused from the header | refused before allocation (mutant killed) |
| Design answers | `sources/cdp.rs` | names 128, sources 512, words 512 characters; lengths ±1,000,000; 64 waiting, 64 elements | a wrong shape refuses the whole answer |
| Page text, history | `sources/browser.rs:143` | 2,048 characters, 100 entries | cut at a character |
| Files the fs source reads | `sources/fs.rs:258` | 2 MiB a file, 10,000 children, 20,000 entries, depth 16 | cut and said |
| Untracked files the git source shows | `sources/git.rs:305` | 2 MiB a file (27) | cut |
| Log files | `sources/logs.rs:281` | 256 KiB chunks | read in chunks |
| Theme, triage, plugin files | `ui/theme/files.rs`, `core/findings/triage.rs:391`, `panes/script_pane/pane/library.rs:645` | 256 KiB, 4 MiB with 10,000 entries, 256 KiB | refused by name; a triage file that does not read is kept aside (mutant killed) |
| Settings file | Rune `Prefs` | none in this repository (26) | a key no setting declares stays |
| docker, git, launchctl, xcrun, curl output | each spawn | 8 MiB, 32 MiB, 1 MiB, 4 KiB, 256 KiB body and 4 KiB stderr | killed with its group |
| Shell output | `sources/pty.rs:318` | 64 KiB reads, parsed per 4 KiB under the lock; 10,000 lines kept | the oldest lines go |
| Daemon flags file | `sources/server.rs:38` | 64 KiB (6) | cut |
| Plugin scripts, their fetches | `rune-luau` sandbox; `library.rs` | watchdog and heap cap; 64 answers, 8 waiting, 32 addresses | stopped at the cap, said |
| Pasted or sent addresses | `core/address/uri.rs` | 2,048 bytes; 12 attached, 8 KiB excerpts | refused |

### Threads: 21 sites

`python3 tools/gate/inventory.py thread`

| Site | Owns | Stops when |
|---|---|---|
| `app/start.rs:389` | one source (its ask receiver, feed sender) | its ask channel closes (`Close`, quit); quit waits up to 3 s (4) |
| `core/wire/client.rs:127` | the supervisor: socket writer, queue | `close(grace)` or the stop flag; quit waits 2 s |
| `core/wire/client.rs:324` | the socket reader | the socket is shut down; joined by the supervisor |
| `dap/client.rs:167`, `:178` | the adapter's stdin, stdout | the writer's sender drops with `Link`; the reader at end of file after the group kill |
| `dap/client.rs:259` | xcrun's stdout (4 KiB) | end of file |
| `dap/client.rs:276`, `:295` | ask relays | their sender closes; bounded (7) |
| `docker.rs:123` (×2 a run) | one pipe each, 8 MiB | end of file, at the latest after the group kill |
| `fetch.rs:169`, `:177` | curl's stderr (4 KiB), stdout | end of file; curl always stopped |
| `git.rs:70` | git's stdin then stdout | end of file |
| `launchd.rs:78` | stdout, 1 MiB | end of file |
| `lsp/client/server.rs:176`, `:178` | the server's stdin, stdout | the queue's sender drops; end of file |
| `lsp/client.rs:90` | the source's ask relay | its commands close |
| `pty.rs:252`, `:266` | the PTY reader and reaper; the writer | end of file then reap; the writes' sender drops |
| `server.rs:79` | the detached daemon's reaper | the daemon exits (never joined, by design) |
| `ui/code/colour.rs:192` | one text to colour | done; never joined, owns only its text |
| `ui/disk.rs:23` | the app's file writes | `Disk::finish` at quit joins it |

### Locks: 19 sites

`python3 tools/gate/inventory.py lock`

| Lock | Held by | Across |
|---|---|---|
| `pty::Screen.parser` | PTY reader; UI draw and resize | one 4 KiB parse; one screen copy (19) |
| `pty::Screen.lines` | PTY reader; UI draw | a push; never with `parser` held |
| `pty::Shell.child` | reaper; `stop` | `child.wait()` in the reaper (18); one signal in `stop` |
| `settings::pane::file` `prefs` | disk worker; UI at start | the settings write (worker only); takes `pending` inside it, the UI takes `pending` alone, so no cycle |
| `settings::pane::file` `pending` | UI; worker | a map insert |
| `AtomicBool` stop (wire), two `fetch_add` id counters | — | `Relaxed`, publishes nothing |

### File writes: 7 writers

`python3 tools/gate/inventory.py write`

| Writer | Survives a crash |
|---|---|
| `core/findings/triage.rs:360` `save` | yes: temporary created new, synced, renamed, folder synced (5) |
| `core/findings/triage.rs:419` a bad file moved aside | a rename |
| `sources/lsp/client/edit.rs:144` a quick fix applied | yes: `create_new` temporary, synced, permissions kept, renamed, folder synced |
| `ui/theme/files.rs:487` theme | yes: through `triage::replace` (12) |
| `panes/script_pane/pane/library.rs:655` a plugin's store | yes: through `triage::replace` (12) |
| `panes/script_pane/pane/library.rs:525` a removed plugin's folder moved to the Trash | a rename: whole or not at all, never a delete; no folder sync |
| Rune `Prefs` (settings) | contents whole; the rename may not survive (26) |

## Mutants

The gate: `python3 tools/gate/har.py --selfcheck` passes 26 bad and 9 clean samples; the two new bad samples were each seen passing the old checks (Findings 11).

Six tests across five crates, each run against its code broken (the original kept under `target/mutants/<name>.orig` and copied back and touched after; runner `target/mutants/run.py`):

| Crate | Test | Code broken | Failed |
|---|---|---|---|
| yd-core | `wire::frame::tests::a_line_at_the_cap_reads_and_one_byte_more_is_skipped` | `read_frame` reads twice the cap | yes: "the tail of a long line must not be read as a frame" |
| yd-core | `a_table_lets_its_oldest_rows_go_at_the_cap` (`tests/contracts.rs`) | `Table::trim` at twice `MAX_ROWS` | yes |
| yd-sources | `lsp::client::tests::a_message_past_the_cap_ends_the_server_with_an_error` | `read_message` with no body cap | yes |
| yd-ui | `screen::surface::tests::a_frame_one_row_past_the_pixel_cap_is_refused_for_its_size` | `MAX_PIXELS` plus a row | yes |
| yd-panes | `design::pane::tests::a_bad_colour_is_refused_in_place_and_a_good_one_is_asked_of_the_page` | `design::colour` accepts any text | yes: "nothing is applied" |
| yi-desktop | `shell::view::tests::kept::a_triage_file_that_did_not_read_is_never_written_over` | `triage::open` never moves a bad file aside | yes |

And the new git test against each of its two controls removed in turn: failed both times.
