# Sandbox access model: localhost egress and a grant-based passthrough

```
status:  PROPOSAL, 2026-09-26, revised after two review passes: three refuters (security,
         usability, minimalism), then five lenses with a refute of their merged list
         (grounding, security, usability, minimalism, precedent). Design only; lands by the
         owner's call. Decided in five AskUserQuestion rounds (§8, verbatim). Method: the
         yi-ideate skill.
tree:    origin/main ab8f7817 (0.329.0, last decision row D249). Open PRs claim versions up to
         0.339.0 and rows up to D252. Draft rows start at D253; renumber at landing after
         re-reading the header and every open PR.
issues:  stage 0: #580, #583, #584 (filed 2026-09-26 at the owner's call). One issue per
         remaining PR, milestone "Runtime, kernel and prompts", opened after approval.
marks:   ✓ exists on main · ✚ new in this plan
amends:  D87, D205, D206, D207, D216, D241.
```

## 0. Summary

The owner, 2026-09-24:

> "a kernel cell should be able to open outbound connections to localhost"

> "this sandbox is quite restricting. we shoukd put more thought into a generic passthrough
> method thats secure" <!-- codespell:ignore thats -->

The plan adds no primitive. It generalizes the two that already govern access and compiles
them at three enforcement points:

| primitive | what it is | status |
|---|---|---|
| **Wall** | what work may not touch: `deny_read`, `deny_write`, `deny_url`; inherited downward, only ever narrowing (seven-primitives §3.7) | ✓ `crates/runtime/src/wall.rs`; enforced at the tool seam only |
| **Grant** | what a holder may touch beyond the base profile; a kept rule is the existing `SessionPermissionRule` with new scope identities | ✓ D207 (`crates/permission/src/rules.rs`), memory-only today |

A spawn's access is one expression, evaluated wherever access is enforced:

```
access(holder) = (base ∪ grants↓(holder)) ∖ wall↓(holder)
```

| enforcement point | decides | enforces | status |
|---|---|---|---|
| tool seam (`Wall::check`, the broker) | per tool call | paths and URLs a tool names | ✓ |
| OS profile (Seatbelt on macOS, Landlock on Linux) | at spawn, fixed for the process's life | files, loopback | ✓ Seatbelt; ✚ Landlock; ✚ grants and wall compiled in |
| egress proxy | per connection, live | every non-loopback host, for clients that honor proxy variables | ✚ |

What changes, one line each:

1. Stage 0 closes five live holes before anything widens: the session corpus, the global
   harness and the MCP session store are writable across the sandbox boundary (the last one
   runs its commands on the host), secrets are readable, and a juror's wall does not reach its
   cells.
2. Every contained spawn gets loopback: bind, inbound and outbound to `localhost:*` (quote 1).
3. Approving a sandbox refusal widens the next spawn by the refused path; the sandbox stays on.
   Today an approved call runs with no sandbox.
4. Grants are ledger entries, flow down only, and survive `--continue`.
5. Non-loopback traffic from proxy-aware clients goes through a host proxy that authenticates
   each spawn and decides per domain, live, so the long-lived kernel gets a new domain without
   a restart.
6. Linux gets Landlock filesystem rules; the host verifies git dirs before its own git runs.

## 1. The problem

### 1.1 What exists

- One `Sandbox { writable, deny_read, deny_write, loopback }`
  (`crates/tools/src/sandbox.rs:12-20`) compiles to SBPL on a `(deny default)` base
  (`vendor/seatbelt/seatbelt_base_policy.sbpl:8`) and is applied as a `sandbox-exec -p` argv
  prefix (`wrap`, `sandbox.rs:159-168`; the kernel's variant is `kernel_prefix`, `:98-105`). `available()` is macOS-only (`:54-56`); Linux runs unsandboxed.
- The only network rules are loopback bind and inbound, emitted when `loopback` is set
  (`sandbox.rs:75-88`), which only the kernel profile sets (`:90-96`); `for_workspace` sets it
  false (`:50`). No profile has an outbound rule. D87: "no outbound rule, so a cell reaches no
  local service either".
- The kernel's profile is the workspace profile plus `~/.yi/harness`, `~/.yi/mcp` and the
  family board (`crates/runtime/src/kernel.rs:211-230`, D240). Its `bash()` jobs run under that
  profile, captured once (`kernel.rs:35-40`, `wiring.rs:162-167`, D241).
- A Seatbelt profile cannot change once applied: a nested `sandbox-exec` fails with
  `sandbox_apply: Operation not permitted`, widening or narrowing (probed). `kernel_wrap`
  recomputes the profile at every kernel boot (`kernel.rs:319-328`); `set_sandbox`
  (`:309-317`) has only a test caller, and a kernel restarts only after a hang or a crash.
- The bash tool's passthrough (D206, D207): a contained run whose failure looks like a
  sandbox refusal (`denial_hint`, `sandbox.rs:207-233`, which returns nothing on exit 0,
  `:217`) records its program and verb; the next such call asks
  (`crates/runtime/src/permission.rs:335-362`), and an approved ask returns
  `contained: false` (`permission.rs:688-693`), which the tool loop honors by running with no
  sandbox (`crates/runtime/src/tools.rs:245-275`).
  Subtree grants exist only for path tools (`tree_writes` requires `command.is_none()`,
  `rules.rs:247`; `extract_targets` at
  `permission.rs:82-108` reads `path` and patches); a bash call is offered its program and
  verb, and a kept grant runs uncontained too.
- Kept rules live in `SessionRules`, in memory. `SessionRules::load` and `insert`
  (`rules.rs:117`, `:143`) exist; `load` has only test callers.
- The kernel has no ask-on-refusal path: the containment-failure memory keys on bash
  command scopes, a refused cell is a bare `PermissionError`, and neither cells nor kernel
  `bash()` jobs get `denial_hint`.
- Headless: with no asker an ask is a deny (`permission.rs:663-678`). Evals and harbor run
  `--yolo`, so their bash is uncontained; on macOS their kernel is still Seatbelted, and on
  Linux it is uncontained with an open network.
- Children share the parent's broker (`..wiring.clone()` at `wiring.rs:48`; the field at
  `:66`), so a grant a child's ask produced
  applies to the whole family. The wall is tool-seam only (`wall.rs:116`, "not a sandbox";
  `Wall::check` reads `path` and `command`), so a walled juror's Python cell can write the tree.

### 1.2 Evidence

Session mining, read-only over `~/.yi/sessions` (436 JSONL files, 2026-08-24 to 2026-09-25;
30 contain bash or ipython calls):

- 250 bash calls and 54 ipython cells. Replaying today's `safety::verdict`: 61 allowed, 183
  contained, 6 asked (`xargs` 4, `rm` 2; two denied headless). Asks per session: median 0,
  maximum 1.
- Three sessions contain a sandbox `Operation not permitted`, 12 distinct refusals, 10 of them
  in one session on 2026-09-14 that predates D205 (0.260.0, 2026-09-15). What remains after
  D205: writes into a sibling worktree, a `.git/config` write (D205 by design), a test's
  Unix-socket bind, a dist build's target dir. 10 of the 12 refusals name the refused path in
  the command's output.
- No egress of any kind: no package install, no git fetch or push, no curl, no network from a
  cell. The model is told "no network, no socket bind" (`crates/permission/src/decide.rs:26`,
  `crates/runtime/src/prompts/identity.md:8`, `prompts/doctrine.md:198-199`) and does not
  try.
- 132 of the 250 calls end in `| head`, `| tail`, `; echo` or `|| true`. One of the
  refusals, a `git worktree add … | tail -3`, exited 0, so no hint and no ask fired.

The corpus cannot measure this design: it has no egress, and the one workflow the owner
named is absent. The case rests on the owner's two quotes and the threat model (§6). Ask
fatigue is measured after stage 2 from the ledger, not estimated.

A Yi agent's probe session on 2026-09-25 (the tool-ergonomics report, intake #565-#573) hit
the wall the same way: `git worktree add -b … ../ergonomics origin/main` could not write
`.git/config` or create the sibling directory, half-succeeded, and the agent reported success.

### 1.3 Live defects (stage 0)

1. **The session corpus is writable** from every root kernel and every contained bash run
   (#580). `session_dir` is `~/.yi/sessions` (`crates/cli/src/main.rs:480`, `:502`,
   `:715-725`); the root kernel's writable root is the same directory (`wiring.rs:171-178`).
   Every session's `<id>.kernel-state.dill` sits flat there (`kernel.rs:192-207`) and is
   restored with dill at that session's next boot: a cell in A plants code that runs in B's
   kernel under B's workspace, or rewrites B's transcript.
2. **The global harness is kernel-writable** (#583). `~/.yi/harness` is a writable root
   (`kernel.rs:220`) and the global harness state dir (`:349-353`): skills, subagents,
   memories and prompt notes other sessions load.
3. **The host runs commands from a sandbox-writable MCP session store** (#584).
   `fetch("mcp://<name>/…")` is a read, so Auto mode does not ask, and it makes the unsandboxed
   host run `yi mcp --json @<name> resources-read …` (`McpOneShot`,
   `crates/cli/src/main.rs:583-598`). `@<name>` is looked up in the session store,
   `~/.yi/mcp/sessions.json` (`crates/mcp-cli/src/sessions.rs:24`, `:57-60`, `:129-131`), and
   the stored spec is run as is (`do_session_op`, `crates/mcp-cli/src/lib.rs:200-230`). That
   directory is a kernel writable root (`kernel.rs:221`). A cell writes a stdio spec with any
   command into `sessions.json`, fetches `mcp://<name>/x`, and the host runs the command
   outside every sandbox with no ask. `resolve_server` and the workspace `.mcp.json` files
   (`crates/mcp-cli/src/config.rs:115-123`) are reached only by `connect`, which writes the
   store; a contained `connect` is the other way to plant an entry.
4. **Secrets are readable.** `deny_read` hides `.ssh`, `.gnupg`, `.aws`, `.kube`, `.docker`
   (`sandbox.rs:24`). `~/.yi/config.json` (the provider `keys`), `~/.config/fgj`,
   `~/.config/gh`, `~/.config/gcloud` and MCP token files are readable from a cell.
5. **Every kernel's ZMQ key is readable** in `$TMPDIR/yi-kernel-*/connection.json`
   (`crates/kernel/src/connection.rs:57-78`). Harmless until loopback egress; fixed in stage 1.
6. **Prompts overstate the kernel.** `ipython.rs:47` says "`%pip install x` adds a package" and
   `ipython.rs:126` says "Run `%pip install {name}`"; both fail on macOS (no network, a
   read-only venv).

## 2. Laws

- The owner, verbatim, seven-primitives §2 law 7: "start with a permissive model along the
  'anything with a lease' and when we start running into permissions/authentications
  oversteps, then we add scopes". The two quotes in §0 are that overstep; a grant scope is
  the scope being added.
- One ledger: "the jsonl ledger is the ledger. dont introduce multiple sources of truth". Kept
  grants are entries in the session JSONL, and the rule set is their fold.
- The wall only narrows and is inherited downward; grants flow the same way, never up.
- Every outward action is visible: an egress decision is a ledger entry.
- Triggers read data: nothing asks a model whether access is safe.

## 3. The primitives, extended

### 3.1 Wall ✓

Unchanged in shape. Two changes in reach, both only narrowing:

- The OS profile subtracts the wall (stage 0): `exec_sandbox` and the kernel's profile add
  the wall's `deny_*` (`wiring.rs:163-167`). A juror's `deny_write: ["."]` then holds for its
  Python cells.
- The egress proxy applies `deny_url` with the wall's own prefix match (stage 3), so a juror's
  `DENY_URL` (`plan/judge.rs:28-40`) denies its cells the network.

### 3.2 Grant ✓, generalized

A kept grant is the existing `SessionPermissionRule` (`crates/types/src/permission.rs:22-29`).
What is new:

- **Scope identities** beside `dir_identity` and `scope_identity` (`rules.rs:225-240`):
  `write_identity(canonical subtree, program verb)` for bash, and
  `egress_identity(holder, domain)`. Allow-once is the existing `AllowOnce` and keeps nothing.
- **Bash write targets** ✚: the refused path is read from the contained run's output (EPERM
  text naming a path), whatever the exit code. The offer is its parent directory, then the
  tree root, as D207 offers path tools. A refusal that names no path escalates once.
- **Ledger** ✚: each kept rule is appended as `Entry::Custom { custom_type:
  "permission_rule" }` through `append_custom` (`crates/session/src/store.rs:185`), as the
  lease does (`crates/runtime/src/lease.rs:158`); `SessionRules::load` replays them on
  `--continue`. The refusal itself is already in the JSONL as the tool result.
- **Flows down only** ✚: a child starts with a copy of its parent's rules and keeps its own;
  nothing is written back. Children stop sharing the parent's rule set.
- **Protected targets**: the host-run git paths (D205), the session corpus other than the
  holder's own state, the global harness, the MCP session store, the secret list (§5.3),
  `~/.yi/config.json`, the daemon socket. They never widen. A refusal on one asks; approved
  once, that call runs uncontained. For secret reads and ssh, "always" keeps today's D207
  program-and-verb escalation for the session (owner, round 5), so `just pr` asks once; for
  git host-run paths and the corpus, nothing is ever kept.
- **Listing** ✚: `/permissions` today only gets and sets the mode
  (`crates/runtime/src/slash.rs:134-150`); stage 2 adds the kept rules to its output.

### 3.3 Enforcement points

- **OS profile** ✓: compiled at spawn. Bash calls and kernel `bash()` jobs compile their own
  profile at each spawn and take a new grant at once; `register_exec` stops capturing a fixed
  sandbox. The kernel process compiles at boot (`kernel_wrap`, FREE); since it restarts only
  after a hang, a refused cell's message says the kernel keeps its boot profile and to do the
  write with `bash()` or the bash tool.
- **Egress proxy** ✚: HTTP CONNECT on tokio in the session's host process, `127.0.0.1:0`; no
  new crate. The host mints a token per spawn and passes it as proxy credentials in the
  proxy variables (§5.5), with `NO_PROXY` set to loopback; the token maps to (session,
  holder) and dies with the spawn. The proxy asks the
  broker once per new domain, so a domain grant takes effect on the next connection, the
  kernel's included.

## 4. Composition

| capability | composed from | status |
|---|---|---|
| a cell reaches a dev server or database on localhost | base profile: loopback bind, inbound, outbound | ✚ one SBPL rule |
| a juror's cell cannot write or fetch | wall subtracted in the profile and at the proxy | ✚ FREE once compiled |
| `cargo fmt` in a sibling worktree | refusal names the path → ask → `write_identity` → next spawn's profile | ✚ |
| `uv`, `cargo fetch`, scripts and the kernel reach a registry | proxy + seeded domains | ✚ |
| a new domain in a long-running kernel | proxy holds briefly, broker asks once, kept rule | ✚, no restart |
| `just pr` (fgj reads a hidden token, git push over ssh) | escalate, "always" kept for the session | ✓ D207, filtered |
| grants survive `--continue` | `permission_rule` entries, `SessionRules::load` | ✓ load, ✚ entries and caller |
| a child does not widen its parent | rules copied at spawn, never written back | ✚ |
| headless run | no asker → deny | ✓ |
| audit | `permission_rule` entries, first egress decision per (holder, domain), tool results | ✚ |
| Linux parity | the same `access(holder)` compiled to Landlock | ✚ |
| a contained git cannot plant host-run config | Seatbelt deny on macOS ✓; host verification before its own git ✚ | ✓ / ✚ |
| a child run in a container (#447) | the same `access(holder)` compiled to mounts and the same proxy | DEFER |

Unchanged by design: mode-level asks (git fetch, pull, push, clone; pip and npm install;
`decide.rs:26`, `safety.rs:282-287`, `:389-391`) still ask at the gate, and an approved one
runs uncontained, never reaching the proxy.

## 5. Topics

### 5.1 Loopback

Every contained spawn (kernel, its `bash()` jobs, the contained bash tool) gets one network
section, and `Sandbox::loopback`, `kernel_policy` and `kernel_prefix`'s special case are
deleted:

```
(allow network-bind     (local ip "localhost:*"))
(allow network-inbound  (local ip "localhost:*"))
(allow network-outbound (remote ip "localhost:*"))
```

Probed facts (2026-09-26):

- A loopback connect succeeds and `1.1.1.1:443` stays refused. Seatbelt rejects any host other
  than `*` or `localhost` at load time.
- `localhost` means every address the host owns: `127.0.0.1`, `::1`, and the machine's LAN
  interface addresses. Other hosts on the LAN stay refused.
- `localhost:*` does not open Unix sockets, which need `(remote unix-socket …)`, so the daemon
  socket stays closed.
- **No port can be subtracted.** After `localhost:*`, a port deny is ignored in every order and
  `require-not` form; only a port-specific allow restricts. A forward proxy the user runs on
  loopback (Clash, Surge, Squid, mitmproxy) is therefore an egress path around the domain
  policy. The owner accepted this (round 5); `yi doctor` lists loopback listeners so one is
  visible.

What loopback exposes is the user's own local services. A contained spawn with loopback is
still narrower than an approved bash call, which runs with no sandbox. The yi-owned exposure,
other kernels' ZMQ, closes in the same PR: connection files move under a host-owned run
directory in every profile's `deny_read`, and each kernel's profile re-allows its own file.
Seatbelt file rules are last-match-wins, so a literal allow after a subpath deny works
(probed).

### 5.2 Passthrough

| refused target | approval does | "always" keeps |
|---|---|---|
| a path outside the roots, not protected | widen: the parent (or tree root) added to that program and verb's next spawn | a `write_identity` rule |
| a secret read or an ssh connection | escalate: this call runs uncontained | the D207 program-and-verb escalation, for the session |
| a git host-run path or the session corpus | escalate once | nothing |
| a non-loopback host from a proxy-aware client | the proxy asks for the domain | an `egress_identity` rule |
| a non-loopback host from a client that ignores the proxy | refused; the hint says so | nothing |

Detection: for contained runs, `denial_hint` matches the refusal text whatever the exit code,
and also matches resolver failures (`nodename nor servname provided`), where the hint says the
client ignores `HTTPS_PROXY`. Seatbelt reports a bare errno and cannot name a host, so a
network refusal that names no path gets its own hint: a raw socket to a non-loopback host
cannot be granted; use a client that honors `HTTPS_PROXY` (before stage 3: non-loopback
network is unavailable).

The hint's second sentence changes with stage 2. Today it says the next contained call asks
and then runs uncontained (`sandbox.rs:229-231`); after stage 2 it says an approval widens the
next spawn by the named path, or, for a protected target, runs that one call uncontained.

### 5.3 Secrets

Stage 0 grows `CREDENTIAL_DIRS` (`sandbox.rs:24`) into a list of files and directories:
`~/.yi/config.json`, MCP token files, `~/.config/gh`, `~/.config/fgj`, `~/.config/gcloud`,
`~/.netrc`, `~/.git-credentials`, `~/.npmrc`, `~/.pypirc`, `~/.cargo/credentials*`. Where a
secret sits under a writable root (MCP tokens under `~/.yi/mcp`), it goes in `deny_write` too.
A deny list is incomplete by nature; a user-extendable list is deferred until someone needs
it.

### 5.4 Kernel

- Boots with `(base ∪ grants↓) ∖ wall`, recomputed at each boot (`kernel_wrap`). Its jobs
  compile per spawn. `set_sandbox` is deleted.
- `%pip install` needs a writable site dir as well as the network: `PIP_TARGET` and
  `PYTHONPATH` point at the session's own state directory from #580 (per session, since a
  package shared across sessions is a code-injection channel between them).
- Refusals in cells and jobs get `denial_hint`.

### 5.5 Egress proxy

- HTTP CONNECT only; the CONNECT line carries the name, so the proxy resolves it and the
  sandbox never needs DNS.
- A CONNECT to an IP literal is refused unless a rule names that address. A CONNECT whose
  name resolves to a loopback, link-local (including `169.254.169.254`), private (RFC 1918,
  `fc00::/7`) or carrier-grade NAT address is refused as well: the proxy runs on the host and
  would otherwise reach what Seatbelt does not grant (a DNS name for the metadata service, a
  LAN router). The check runs on the address actually dialed, so a rebinding name cannot
  switch between check and connect. Fronting through an allowed CDN is a stated residual.
- Variables: the spawn gets the proxy URL under every key package managers read, taking
  Codex's list as the floor (`PROXY_URL_ENV_KEYS`,
  `ref/agents/codex/codex-rs/network-proxy/src/proxy.rs:547-564`): `HTTP_PROXY`,
  `HTTPS_PROXY`, `ALL_PROXY` (and lowercase), `WS_PROXY`, `WSS_PROXY`, `YARN_HTTP_PROXY`,
  `YARN_HTTPS_PROXY`, `NPM_CONFIG_HTTP_PROXY`, `NPM_CONFIG_HTTPS_PROXY`, `NPM_CONFIG_PROXY`,
  `BUNDLE_HTTP_PROXY`, `BUNDLE_HTTPS_PROXY`, `PIP_PROXY`, plus `NODE_USE_ENV_PROXY=1`, without
  which Node's built-in fetch ignores the proxy.
- If the proxy is not listening (it failed to bind, or the host is shutting down), a contained
  spawn fails with that reason. It never starts without the proxy variables, which on Linux
  would send clients straight out with no domain decision.
- Identity is the per-spawn token. Without it, `localhost:*` lets any sandbox on the machine
  use any session's proxy. On macOS a sandbox cannot read another process's environment
  (the base policy grants no `kern.procargs2`; probed). On Linux `/proc/<pid>/environ` is
  readable by the same user, but Linux's network is open anyway (§5.7).
- If the user has an upstream proxy (`HTTPS_PROXY` in yi's own environment, read at
  `crates/cli/src/main.rs:365-366`), the proxy dials through it.
- Policy: `~/.yi/config.json` key `sandbox.egress` seeds pypi.org, files.pythonhosted.org,
  crates.io, index.crates.io, static.crates.io, static.rust-lang.org, registry.npmjs.org,
  github.com, codeload.github.com, objects.githubusercontent.com,
  release-assets.githubusercontent.com, raw.githubusercontent.com. Any other domain: one
  pending ask per (holder, domain); the proxy holds a connection at most 10 s, then answers
  403 with a body naming the domain and the open ask, and the answer applies to later
  connections. Headless denies. `deny_url` from the wall is checked first.
- `--yolo` allows every domain and records it, so harbor and evals do not regress.
- The first decision per (holder, domain) is a ledger entry.

### 5.6 Headless and CI

No asker means deny, as today. `--yolo` is unchanged, and on Linux its kernel stays
uncontained. Pre-declared grants (`--grant`, a config list) are deferred until a headless,
non-yolo run needs one.

### 5.7 Linux

- `Sandbox::wrap` re-executes `yi` as the wrapper (the Codex `codex-linux-sandbox` arg0
  precedent, `ref/agents/codex/codex-rs/linux-sandbox/`; Codex's default isolation there is
  bubblewrap, with Landlock as its legacy path, `sandboxing/src/landlock.rs`): set
  `no_new_privs`, apply a Landlock ruleset, exec. Same argv-prefix seam as `sandbox-exec`. The entry lives in a new
  module (`crates/cli/src/main.rs` is at 1117 of its 1200-line cap).
- Every crate is `#![forbid(unsafe_code)]`, so this takes the `landlock` crate. The deps
  budget is at its ceiling (`scripts/guardrails/baselines/deps_budget.json`: 20 direct, 167
  transitive; 19 and 167 used). In the same commit as the crate: a YI_DESIGN §18.3 row, a
  `deny.toml` entry where a ban or wrapper applies, the budget raise and a size-ledger row.
- Writes: writable roots ∪ write grants. Reads: Landlock rules only add, so the wrapper allows
  named system roots plus every child of `$HOME` except the secret list, enumerated at spawn
  (281 entries on the owner's machine; negligible). The system roots are `/usr`, `/bin`,
  `/sbin`, `/lib`, `/lib64`, `/etc`, `/opt`, `/nix` and `/dev` (with write on `/dev/null`,
  `/dev/tty`, `/dev/zero`, `/dev/urandom`), `/proc` and the toolchain homes (`CARGO_HOME`,
  `RUSTUP_HOME`, the kernel venv) where they sit outside `$HOME`. Never `/`, `/home` or `/root`
  unless it is `$HOME` itself: allowing a parent of `$HOME` makes the secret carve-out
  impossible.
- Network: not handled (the owner chose filesystem rules only). Linux network stays open, and
  every description of the sandbox says so.
- D205 cannot hold under Landlock (`packed-refs.lock` and `HEAD.lock` need the git dir
  writable, which lets a contained git replace `config` or a hook). The host verifies: it
  records a hash of each git dir's `config` and `hooks/*` and checks it before every git the
  host itself runs, restoring and refusing on a mismatch, with a ledger entry. Checking before
  the host's own git, not only after a contained spawn, covers another session's contained git
  on a shared `.git` (55 worktrees share one here; `crates/runtime/src/lane/settle.rs:125-126`
  runs host git against it). One lock on the common dir is held across the verify and the host
  git, so two sessions' verifications and restores cannot interleave; and every host git runs
  with `-c core.fsmonitor=false -c core.hooksPath=<an empty host-owned dir>`, which closes the
  two common vectors for a contained writer that races the gap between verify and exec. The
  remaining residuals are that gap for other config keys, and the user's own terminal git.
  Which platforms run it is open question 1.
- Availability: the `unshare_works` probe pattern (`crates/tools/src/document.rs:663-675`)
  tests the Landlock ABI once; where it fails, contained means ask (today's behavior,
  `gate.rs:138`), and `yi doctor` says the sandbox is absent.

### 5.8 Monty (evaluated, not planned)

pydantic/monty is an MIT, Rust, Python-subset interpreter with no ambient authority: files,
env, sockets and processes exist only through host functions and mounts; it suspends at each
host call and can dump its state to CBOR there. Evaluated for this plan and recorded, not
planned (owner, round 4).

- It cannot replace the kernel: the subset rejects class inheritance, generators, `match`,
  `del` and method decorators at parse time, and has no third-party packages, no C
  extensions, and no `subprocess`, `socket` or `threading`. `bash()`, `%pip`, dill namespaces
  and `kernel://` assume CPython.
- Where it would fit: walled-juror cells, as a language-level wall that holds the same on every
  platform; host-brokered effects with a mid-flight ask and resume, which a Seatbelted CPython
  process can never do; a snapshot whose restore grants nothing, unlike dill.
- Costs: young; its heap arena is the security-critical code; in-process execution loses crash
  isolation (a fault would take down the yi process holding every session), so it would run
  only in pooled worker processes under the strictest OS profile; a new dependency with unsafe
  code, against a dependency budget at its ceiling; models trained on CPython hit its parse
  errors.
- What would reopen it: a replay of the mined `ipython` cells through Monty's parser showing
  the subset accepts what jurors write.

## 6. Threat model

Adversary: text the model read (a web page, a file, a tool result) steering a tool call, a
cell or a bash command, or a child steered the same way. Out of scope: a malicious local user,
a compromised yi binary.

| # | threat | scenario | mitigation | stage | residual |
|---|---|---|---|---|---|
| T1 | cross-session code injection | a cell in A writes `<B>.kernel-state.dill`; B's next boot runs it | sandboxes write only their own session's state | 0 (#580) | none known |
| T2 | cross-session harness injection | a cell writes a global skill or prompt note every later session loads | global harness writes go through the host | 0 (#583) | none known |
| T3 | host runs a planted MCP command | a cell writes a stdio spec into `~/.yi/mcp/sessions.json` (a kernel writable root), then fetches `mcp://evil/x`, a read that does not ask; the host runs the command unsandboxed | the session store is a protected target no sandbox can write; a kernel-side `connect` goes through the host; the host spawns only from sessions it created from `~/.yi/mcp.json` or trusted, hash-pinned workspace config | 0 (#584) | none known |
| T4 | cross-session kernel control | with loopback, a cell reads B's ZMQ key and sends signed requests to B's kernel | connection files hidden from every sandbox; own file re-allowed | 1 | none known |
| T5 | secret exfiltration | a cell reads `~/.config/fgj` and sends it out | secret list in `deny_read`; domain policy | 0, 3 | deny list incomplete; GET paths reach allowed servers' logs; a user's loopback forward proxy bypasses the policy |
| T6 | confused deputy on loopback | a cell drives Chrome DevTools on 9222, an unauthenticated Redis, a tokenless Jupyter | accepted with `localhost:*` (rounds 1 and 5); `yi doctor` lists listeners | 1 | the user's local services, on every address the host owns |
| T7 | proxy grant confusion | session A's cell uses session B's proxy and rules | per-spawn token → (session, holder) | 3 | Linux `/proc` environ (network open there anyway) |
| T8 | upward widening | a child's kept rule covers its parent | rules copied at spawn, never written back | 2 | none known |
| T9 | self-granting by config | a cell writes a file that grants itself | kept rules only from the host's own ledger entries | 2 | none known |
| T10 | path races and links | a granted `x/` is swapped for a link into a protected path | canonicalized when kept and again when compiled; protected canonical paths refused; Seatbelt checks the resolved path at the syscall; `resolve_aliases` refuses deep links (`sandbox.rs:171-190`) | 2 | Landlock resolves at ruleset time: a swap between compile and exec (test) |
| T11 | host-run git config | a contained git writes `core.fsmonitor`; a host git runs it | Seatbelt deny (macOS); on Linux, host verification under one common-dir lock plus `-c` overrides on every host git | 4 | a contained writer racing verify-to-exec for other keys; the user's terminal git on Linux |
| T12 | the proxy as a reach extender | CONNECT to a raw address, a name resolving to `169.254.169.254` or a LAN address, or fronting through an allowed CDN | IP literals refused; loopback, link-local, private and CGNAT resolutions refused on the dialed address | 3 | CDN fronting |
| T15 | proxy absent | the proxy fails to bind and a spawn starts without its variables; on Linux clients go straight out | a contained spawn fails when the proxy is not listening | 3 | none known |
| T13 | silent degradation | Landlock missing in a container | ABI probe; contained means ask; `yi doctor` | 4 | none known |
| T14 | ask fatigue | the human rubber-stamps | subtree and domain scopes, seeded registries, "always" for escalation of secrets and ssh | 2, 3 | measured from the ledger after stage 2 |

## 7. Exists and new

| piece | status | where |
|---|---|---|
| Seatbelt compiler, `-D` params, resolved aliases | ✓ | `crates/tools/src/sandbox.rs` |
| kernel profile, recomputed per boot | ✓ | `crates/runtime/src/kernel.rs:211-230`, `:319-328` |
| refusal hint and program-and-verb memory | ✓ | `sandbox.rs:207-233`, `permission.rs:160-164`, `safety.rs:487-514` |
| kept rules, scope identities, `load` | ✓ | `crates/permission/src/rules.rs:117-305` |
| allow-once runs uncontained | ✓ | `permission.rs:688-693` |
| ledger custom entries | ✓ | `crates/session/src/store.rs:185`, used by `lease.rs:158` |
| wall | ✓ | `crates/runtime/src/wall.rs` |
| headless deny; contain-to-ask without a sandbox | ✓ | `permission.rs:663-678`, `gate.rs:138` |
| availability probe pattern | ✓ | `crates/tools/src/document.rs:663-675` |
| `/permissions` (mode only) | ✓ | `crates/runtime/src/slash.rs:134-150` |
| kept rules listed by `/permissions` | ✚ | `crates/runtime/src/slash.rs` |
| per-session state, host-routed harness, a protected MCP session store | ✚ | #580, #583, #584 |
| wall and secret list in the profile | ✚ | `wiring.rs:163-167`, `sandbox.rs` |
| one loopback section; own-connection-file re-allow | ✚ | `sandbox.rs`, `crates/kernel/src/connection.rs` |
| bash write targets from refusal text; `write_identity`, `egress_identity` | ✚ | `permission.rs`, `rules.rs` |
| `CallOutcome.sandbox: Option<Sandbox>` in place of `contained: bool` | ✚ | `permission.rs:79`, `tools.rs:271-275` |
| `permission_rule` entries and their replay | ✚ | `permission.rs` |
| egress proxy | ✚ | a new module in `crates/runtime` |
| Landlock wrapper | ✚ | `crates/tools/src/sandbox.rs` (Linux arm), a new `crates/cli` module |
| git-dir verification | ✚ | host side, before host-run git |

## 8. Decisions log

Each round was an AskUserQuestion; the owner's selections are quoted as chosen.

| round | fork | owner's answer |
|---|---|---|
| 1 | localhost scope | "localhost:*, every contained spawn (Recommended)" |
| 1 | what approval grants | "Widen by the refused capability (Recommended)" |
| 1 | grant store and direction | "Ledger events, flow down only (Recommended)" |
| 1 | the session-corpus hole | "File now, fix as stage 0 (Recommended)" → #580 |
| 2 | non-loopback egress | "Build the egress proxy now" |
| 2 | kernel grants | "Jobs per spawn, kernel at next boot (Recommended)" |
| 2 | Linux | "Landlock fs now" |
| 2 | pressure test | "Three Opus refuters (Recommended)" |
| 3 | proxy identity | "Per-spawn token (Recommended)" |
| 3 | domain policy | "Seed registries, ask the rest (Recommended)" |
| 3 | secrets | "Extend deny_read in stage 0 (Recommended)" |
| 3 | Linux reads | "Enumerate read roots per spawn (Recommended)" |
| 4 | local forward proxies | "Config deny-list of loopback ports (Recommended)"; withdrawn in round 5 |
| 4 | Linux D205 | "Host verifies git dirs (Recommended)" |
| 4 | Monty | asked: "Running in-process loses crash isolation,? what?"; then "Record as evaluated, not planned" |
| 5 | loopback deny-list unbuildable | "Accept, document, show listeners (Recommended)" |
| 5 | what "always" keeps on protected targets | "Session escalation, except git and corpus (Recommended)" |
| 5 | harness and MCP holes | "File both now, into stage 0 (Recommended)" → #583, #584 |

Changed by the refute pass without a new fork: the grant is the existing
`SessionPermissionRule` with new identities, not a new type; ledger entries are
`Entry::Custom`, not `AgentEvent`s (the session crate never writes those); config is JSON keys
in `~/.yi/config.json`; SOCKS5, `SandboxRefused`, `GrantRevoked`, `/grant`, `/grants`,
`--grant` and per-connection ledger lines are cut or deferred; Landlock's degradation is a
`yi doctor` line; `set_sandbox` is deleted. The macOS half of git-dir verification is kept
because round 4 chose it; the minimalism pass found it redundant there (Seatbelt already denies
those writes, `sandbox.rs:28`, `:35-38`), which is open question 1.

Changed by the second review, also without a new fork: #584 targets the MCP session store, not
the workspace `.mcp.json` files; the proxy refuses private, link-local and loopback
resolutions, sets the package managers' proxy variables and fails closed when absent;
stage 1 edits `identity.md` and `doctrine.md` as well; stage 2 rewrites the denial hint; the
Landlock read roots are named; git-dir verification holds a lock and pins two config keys;
`/permissions` listing is new work, not existing; stale cites fixed.

Rejected alternatives:

| alternative | why not |
|---|---|
| per-port loopback grants | a kernel cannot widen without a lossy restart; owner chose `localhost:*` |
| a loopback port deny-list | Seatbelt cannot subtract a port from `localhost:*` (probed) |
| loopback for the kernel only | two network postures; the same need arises in bash |
| approval keeps meaning "no sandbox" | every grant a full escape for its program and verb |
| host-brokered effects as the primary mechanism | one typed verb per effect; used only where the effect is generic, the byte stream |
| per-run grant tokens as authorization | tokens carry identity to the proxy; authorization stays the kept rules |
| family-wide grants | a child's grant widens its parent |
| a new `Grant` type or `AgentEvent`s for grants | `SessionPermissionRule` and `Entry::Custom` exist |
| Seatbelt `*` egress grants | whole-internet exfiltration per program |
| restart the kernel on grant | loses live handles, threads and over-cap variables |
| per-session proxy credential | a rule kept for `pip install` would cover every cell |
| SOCKS5 or a unix-socket proxy | CONNECT carries the name; unix sockets need a TCP shim that reopens identity |
| allowlist reads under `$HOME` | breaks every tool that reads a dotfile |
| Linux reads left open | secrets over an open network |
| bwrap and seccomp (Codex port), network namespaces | user namespaces are often blocked under root CI and in containers; a private loopback breaks quote 1 without a forwarder |
| seccomp user notification | Linux-only; the proxy gives live network asks on both platforms |
| a sandbox violation-log monitor | the refusal text and the proxy name the scope for the cases that matter |
| Monty | §5.8 |

## 9. Build order

Each PR lands with its changelog row and, where it changes a settled decision, its D-row and
ADR. Red-first tests run on macOS with a per-worktree HOME (`env
HOME=~/Development/.yi-homes/<wt> CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup
UV_CACHE_DIR=~/.cache/uv`) from a worktree under `~/Development`, never under a temp root,
where every write is allowed and a sandbox test passes vacuously. LOC estimates are the
minimalism pass's; every crate sits at its shrink-only ceiling, so each PR carries its growth
memo.

### Stage 0: close what is open today (four PRs, any order)

- **#580**: per-session state directory; the corpus is no longer a writable root, and the bash
  tool gets no session dir at all (`main.rs:502` stops passing `&session_dir`). Red: a contained bash run and
  a root kernel cell each create `~/.yi/sessions/<other-id>.kernel-state.dill` (succeeds today).
- **#583**: global harness writes through the host. Red: a cell creates a file under
  `~/.yi/harness/` (succeeds today) while `rlm.harness.create_memory(..., global_=True)` keeps
  working.
- **#584**: the MCP session store leaves every writable root; a kernel-side `connect` goes
  through the host. Red: a root kernel cell writes a stdio entry into
  `~/.yi/mcp/sessions.json` whose command creates a marker outside the tree, then
  `fetch("mcp://<entry>/x")`; the marker must not appear (it appears today, and the fetch does
  not ask).
- **Secrets, wall and prompts**: the secret list; the wall's `deny_*` in every profile; the two
  `%pip` texts stop claiming that install works (`ipython.rs:47`, `:126`); stage 3 is when it
  does. Red: a cell reads `~/.yi/config.json`
  (succeeds today); a walled juror's cell writes the tree (succeeds today).

About +40 lines each; none may grow `kernel.rs` (1178 of its 1200-line cap) before stage 1
shrinks it. Gate: stage 1 waits for all four.

### Stage 1: loopback for every contained spawn

The loopback section for every profile; `Sandbox::loopback` and `set_sandbox` deleted;
connection files hidden with the own-file re-allow; `denial_hint` on cells and kernel jobs,
at any exit code, with the resolver-failure strings and the raw-socket hint; the prompt texts,
which must all say "no non-loopback network" instead of "no network" (`decide.rs:26`,
`builtins.rs:507`, `sandbox.rs:230`, `crates/runtime/src/prompts/identity.md:8`,
`prompts/doctrine.md:198-199`); a `yi doctor` line listing loopback listeners.

Red: a cell connects to a host-run loopback listener, and a contained `curl
http://127.0.0.1:<port>`; both EPERM today. Stay refused: `1.1.1.1:443`, the daemon socket,
another kernel's `connection.json`. About +60 lines; it shrinks `kernel.rs` (1178 of 1200).

Demo: a cell queries a local Postgres or dev server.

### Stage 2: grants widen

`CallOutcome.sandbox` in place of `contained`; bash write targets from the refusal text;
`write_identity` rules compiled into the next spawn; protected targets filtered from the offers,
with "always" keeping the D207 escalation only for secrets and ssh; `permission_rule` entries
and their replay; rules copied to children at spawn; jobs compile per spawn; the kept rules in
`/permissions`; the denial hint's second sentence rewritten to widen-or-escalate.

Red:
1. An approved write to a sibling worktree runs contained with that subtree added; a write to a
   third directory in the same call is refused. Today the approved call runs uncontained.
2. After `--continue`, a kept rule still applies. Today it is gone.
3. A child's kept rule does not reach its parent. Today the broker is shared.
4. A rule whose canonical path is protected, including through a link, is never offered.
5. `git worktree add … | tail -3` in a contained run is detected as a refusal. Today exit 0
   hides it.

About +170 lines. Demo: the Yi agent's `git worktree add` into a sibling directory, asked once
and then contained; its `.git/config` write escalates once.

### Stage 3: egress proxy

The CONNECT proxy, per-spawn tokens, the full proxy variable set, fail-closed spawns, the
upstream-proxy dial, `sandbox.egress` seeds, one pending ask per (holder, domain) with the 10 s
hold, IP-literal and private-resolution refusal, `deny_url`, the `--yolo` allow-all,
first-decision ledger entries, `PIP_TARGET`.

Red: a cell fetches `https://pypi.org/simple/` and `%pip install`s a small pure-Python
package (both fail today). Refused: an unknown domain headless, with the domain named; another
session's token (407); a token after its spawn exited (407); a CONNECT to an IP literal; a
CONNECT to a name that resolves to `127.0.0.1` or `169.254.169.254`; a raw
`connect(("1.1.1.1", 443))`; a spawn while the proxy is down (fails, rather than starting
without the variables).

About +280 lines, in the runtime crate, which is at its ceiling; the growth memo prices it.
Demo: `%pip install` in a cell, and a new domain asked once in a running kernel, no restart.

### Stage 4: Linux

The Landlock wrapper, enumerated reads from the named roots, the ABI probe with its
`yi doctor` line, and git-dir verification before host-run git, under the common-dir lock and
with the two `-c` overrides. Probe first: Landlock works for root inside the forge
runner's containers.

Red (Linux CI): a contained write outside the roots succeeds today; a read under `~/.ssh`
succeeds today; a contained `git config core.fsmonitor <x>` then a host git runs it today.
After: EACCES, EACCES, and the host restores the config and refuses. About +180 lines, plus
the `landlock` dependency with its §18.3 row, `deny.toml` entry, budget raise and size-ledger
row in one commit.

Demo: the forge gate runs one contained suite under Landlock.

## 10. D-rows owed (drafts; renumber at landing)

| row | PR | decision | amends |
|---|---|---|---|
| D253 | #580 | a sandboxed process writes only its own session's state; the bash tool gets no session dir | D87, D241 |
| D254 | #583 | global harness writes go through the host; the kernel's harness root is the session's | D87 |
| D255 | #584 | the MCP session store is outside every sandbox's writable roots; a sandboxed `connect` goes through the host; the host spawns only from sessions it created from `~/.yi/mcp.json` or a trusted, hash-pinned workspace file | (new) |
| D256 | stage 0 | the wall and a secret list are compiled into every OS profile | D87, D216 |
| D257 | stage 1 | every contained spawn gets loopback bind, inbound and outbound; kernel connection files are hidden from other sandboxes; `Sandbox::loopback` and `set_sandbox` are deleted | D87, D241 |
| D258 | stage 2 | an approved refusal widens the holder's next spawn by the refused path; protected targets escalate, kept only for secrets and ssh; kept rules are ledger entries copied downward only | D206, D207 |
| D259 | stage 3 | proxy-aware non-loopback traffic leaves through the host's egress proxy, authenticated per spawn, decided per domain, refusing private and link-local resolutions, and failing closed when absent | D87 |
| D260 | stage 4 | Linux contains with Landlock filesystem rules and reads from named roots; the host verifies git dirs under a common-dir lock before its own git | D205 |

## 11. Open questions

1. Where git-dir verification runs. Recommended: Linux only, with the common-dir lock; macOS
   keeps Seatbelt's deny of `HOST_RUN_BY_GIT` (`sandbox.rs:28`), which already stops the
   write. Alternatives: both platforms with the same lock (round 4's wording; pays for a check
   Seatbelt already makes), or no host verification (reopens round 4 and leaves a Linux
   contained git able to plant `core.fsmonitor`).
2. The ask-fatigue rate after stage 2, measured from `permission_rule` entries.
3. The Linux spawn-time race between compiling a Landlock ruleset and exec (T10).
4. Whether a trusted workspace `.mcp.json` reuses `yi trust`'s grant as is, or pins per entry
   (#584).

## 12. Not building

- A new primitive: Wall and Grant cover it.
- Per-port loopback grants, a loopback deny-list, `*` egress grants, restart-on-grant.
- Unix-socket grants: the daemon and container-runtime sockets are protected.
- A Linux network sandbox (netns, bwrap, seccomp): deferred to #86 and #447.
- SOCKS5, a violation-log monitor, seccomp user notification.
- `--grant`, a standing-grant config list, `/grant`, revocation verbs: until a headless
  non-yolo run or the ledger shows the need.
- Monty (§5.8).
