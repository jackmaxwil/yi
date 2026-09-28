# microsandbox as the inner loop's sandbox: findings, a runner, and a spike first

```
status:  PROPOSAL, revision 1 (2026-09-28). Nothing is installed; every microsandbox claim
         below is read from its docs and source, not measured. Stage 0 is a measured spike the
         owner must approve, because it installs microsandbox and a rustup target.
tree:    main @ 09876e0d, re-read 2026-09-28.
extends: docs/plans/2026-09-26-self-improvement-evals.md (§5.3 cascade, §6.3 gate, §18).
subject: microsandbox 0.7.3 (crates.io, 2026-09-24); the docs already list 0.7.4 (2026-09-25).
host:    this Mac: arm64, macOS 26.7.1 (25G313), Apple M4 Max, 16 cores, 128 GiB, 636 GiB free.
marks:   ✓ exists on main · ✚ new in this plan · ⏸ deferred
```

## 0. Summary

**Recommendation: adopt behind a flag.** Build the inner loop's task format and runner protocol
first. They run on today's plain runner (`evals/run.py`, zero new infrastructure). Add
`evals/drivers/msb_runner.py` as a second runner behind the same protocol once the stage 0
spike passes. Make it the inner loop's default only after an A/A run shows it loses no more
trials than the plain runner.

**The one-line reason:** microsandbox is the only option that makes thousands of unattended
`--yolo` rollouts safe on the owner's Mac, with a VM boundary, deny-by-default egress and an
API key that never enters the guest, at a claimed sub-100 ms boot. But it is a beta carried by
two people, its current design is six months old, its last minor release listed 13 breaking
changes, and the path every yi request would take has an open macOS bug (#1691). So it must sit
behind a protocol that lets us drop it at no cost.

What the sandbox does and does not fix, scored on the owner's five complaints:

| owner's complaint | what fixes it | does the sandbox choice matter? |
|---|---|---|
| 1. unstable environment (pulls time out, sidecars vanish, disk fills; 20 of 42 trials lost before the agent ran, per the owner's brief; `tbv4_sweep.sh:79-94` reruns such trials) | one small pinned base image, pulled once; no compose, no per-task images | yes: both the plain runner and microsandbox fix it relative to TB; microsandbox adds its own beta risk |
| 2. too long and slow (45-50 min trials, 4 h per 12-task call) | ≤5 min tasks | no: task length is a property of the task. The plain runner has no boot; microsandbox claims under a second |
| 3. results too similar (sticky per-task scores) | many tasks, not repetitions | no |
| 4. sample sizes too small (12 tasks) | ~150 mined tasks at 8-16 parallel slots | partly: both run in parallel; microsandbox isolates the slots from each other |
| 5. sandboxes too heavy (1.5 GB image per task plus sidecars) | one shared image | yes: plain runner has none; microsandbox has one shared, copy-on-write image |

The inner loop's speed and power come from the task corpus ("Mine from our failures"). The
sandbox decides isolation, Linux fidelity and stability. That is why the plan builds the corpus
and protocol first and keeps the sandbox swappable.

In the cascade (design §5.3), the inner loop takes the place of S2 (one dev task) and S3 (the
six-task dev screen). S1 stays on the fixtures; S4 validation and the final group stay on TB.

The owner's decisions this plan serves, verbatim:

- Inner-loop tasks: "Mine from our failures": about 150 short (≤5 min) graded tasks mined from
  the failure modes in TB trials and real yi sessions. An LLM drafts them and the owner approves.
- TB's role: "Validation + final only".
- Inner-loop host: "what if we try https://crates.io/crates/microsandbox".

## 1. What microsandbox is and how it works

| part | finding | source |
|---|---|---|
| What | "easy, fast, local microVMs for untrusted workloads"; Apache-2.0; "still **beta software**. Expect breaking changes, missing features, and rough edges." | <https://github.com/superradcompany/microsandbox> README |
| VMM | libkrun, on KVM (Linux), Hypervisor.framework (macOS) or WHP (Windows). The guest runs "its own Linux kernel, supplied by microsandbox (built from libkrunfw), not your host kernel" | <https://docs.microsandbox.dev/security/isolation.md> |
| Devices | only virtio-console, virtio-net, virtio-fs, virtio-blk and virtio-rng: "There is no general-purpose passthrough." | same page |
| Guest agent | `agentd` runs as PID 1 (root) in the guest. The host sends "framed messages on virtio-console" (run this command, read this file, open this TCP connection); the guest "cannot reach back through the channel to run commands on your host" | same page |
| No daemon | "Spawn VMs within your code; no setup server or long-running daemon" | README feature list |
| Runtime files | `msb` plus `libkrunfw` ("the library that bundles the guest kernel") under `~/.microsandbox`; `MSB_AGENTD_PATH` can override the guest agent. Since 0.7.0 "Local creation no longer downloads missing runtimes" | <https://docs.microsandbox.dev/sdk/setup.md>, <https://docs.microsandbox.dev/troubleshooting/macos.md>, <https://docs.microsandbox.dev/migrations/v0.7.md> |
| OCI images | Pulls "the manifest, downloads the layers in parallel, and stacks them as a copy-on-write filesystem". Cache under `~/.microsandbox/cache/` (`layers/` as EROFS, `fsmeta/`, `vmdk/`); layers are content-addressed and shared; a tag "pins the exact layers" on first pull. Each sandbox writes to its own `upper.ext4` | <https://docs.microsandbox.dev/images/overview.md>, <https://docs.microsandbox.dev/sandboxes/lifecycle.md> |
| Boot claim | "Average boot times under 100 milliseconds", footnoted "Boot time refers to guest boot on an M1 machine". That is guest boot only: not the pull, not rootfs assembly, not the host process spawn | README |
| Network stack | "All sandbox traffic flows through a host-side network stack … a user-space stack terminates every packet and checks it against policy before anything leaves". The stack is smoltcp 0.14, compiled into `msb`; DNS is answered at the gateway and forwarded upstream | <https://docs.microsandbox.dev/security/network.md>; `crates/network/Cargo.toml:3`, `engine/dns/interceptor.rs:1-8` at tag v0.7.3 |
| Default egress | public internet allowed; private ranges, loopback, link-local, cloud metadata and the host denied | <https://docs.microsandbox.dev/networking/overview.md> |
| Domain rules | a connection matches a domain rule only "if its destination IP was actually returned as an answer to a DNS query for that domain *from this sandbox*"; SNI must match too; with interception, the decrypted authority must match the SNI (closes domain fronting). Private answers are rewritten to `NXDOMAIN` (rebinding defense) | <https://docs.microsandbox.dev/security/network.md> |
| Strict hostnames | since 0.7, "Strict hostname checking is on by default": a hostname-only allow rule needs TLS interception, otherwise HTTPS to that host is denied | <https://docs.microsandbox.dev/migrations/v0.7.md>, <https://docs.microsandbox.dev/changelog/2026-09-11.md> |
| TLS interception | loads a CA (default `~/.microsandbox/tls/ca.{crt,key}` on the host), "Adds that CA to the guest's trust store", mints a per-host certificate, opens its own verified TLS upstream. Pinning clients must be bypassed; no QUIC/HTTP3 | <https://docs.microsandbox.dev/networking/tls.md> |
| Secrets | the guest gets a placeholder (`$MSB_<NAME>`); the host proxy substitutes the real value only on requests to the bound host, in headers by default. "The real value stays in host memory." Requests carrying the placeholder to other hosts are blocked by default. The CLI "stores an environment reference and reads it at sandbox startup; inline credential values are rejected" | <https://docs.microsandbox.dev/sandboxes/secrets.md>, <https://docs.microsandbox.dev/security/secrets.md> |
| Snapshots | disk snapshots (files and owned volumes; boot a new VM on restore) and full snapshots ("Disk, memory, and running processes"). `msb branch` forks a running or paused sandbox with copy-on-write memory. Local only. "live host connections are not captured" | <https://docs.microsandbox.dev/sandboxes/snapshots.md>, <https://docs.microsandbox.dev/changelog/v0.7.0.md> |
| Host process | one `msb machine` process per sandbox, spawned by the CLI or SDK with its config on fd 96; no daemon (the old `msb server` was removed) | `crates/runtime/lib/client/launch.rs:1-9,31`; `README.md:38` at v0.7.3 |
| Control channel | virtio-console multiport (`agent`, `agent-bulk`), frames `[len u32][id u32][flags u8][body]` with a CBOR body, 4 MiB maximum. Vsock only for user-defined socket routes | `crates/protocol/lib/lib.rs:2,70,74`; `protocol/lib/codec.rs:1-4,28`; `crates/vsock/lib/lib.rs:1` |
| libkrun | a fork, crate `msb_krun =0.1.39` from `superradcompany/libkrun`, compiled statically into `msb` and linked to `framework=Hypervisor`. The kernel, `libkrunfw.5.dylib` (5.6.1), is a separate file loaded with `dlopen` | `Cargo.toml:99-100`; `crates/utils/lib/lib.rs:90-93,179-181`; `crates/runtime/lib/runner/vm.rs:1908` |
| State | `~/.microsandbox` (or `MSB_HOME`): a SQLite database at `db/msb.db` (sea-orm, sqlx), plus `sandboxes/`, `cache/`, `run/`, `tls/` | `crates/utils/lib/lib.rs:19-58,100,170` |
| Root disk | image layers as EROFS images behind one virtio-blk device, with a per-sandbox copy-on-write layer; bind mounts are still virtio-fs passthrough | `crates/runtime/lib/runner/vm.rs:230`; `docs/changelog/2026-04-24.mdx:16`; `crates/filesystem/lib/backends/passthroughfs` |
| CA in the guest | agentd copies the CA into `/usr/local/share/ca-certificates` and `/etc/pki/ca-trust/source/anchors`, appends it to the first of `/etc/ssl/certs/ca-certificates.crt`, `/etc/pki/tls/certs/ca-bundle.crt`, `/etc/ssl/cert.pem`, and sets `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE` to that bundle | `crates/agentd/lib/tls.rs:23-37,76-84,177-200` |
| Secrets imply interception | any secret sets `tls.enabled = true`, and interception then covers port 443 for every host not bypassed; substitution stays limited to the secret's hosts | `sdk/rust/lib/sandbox/config.rs:1051-1057`; `crates/cli/lib/commands/common.rs:2511`; `docs/networking/tls.mdx:190-194` |

Source paths are at tag v0.7.3 (commit `f9f40e1`), <https://github.com/superradcompany/microsandbox/tree/v0.7.3>.

## 2. Platform fit for this Mac

| question | finding | source |
|---|---|---|
| Hardware | "microsandbox local sandboxes on macOS require Apple Silicon"; `uname -m` must be `arm64`. This Mac: `arm64`, Apple M4 Max | <https://docs.microsandbox.dev/troubleshooting/macos.md>; `uname -m`, `sysctl` on this host |
| macOS version | the docs name none. This Mac runs 26.7.1 | `sw_vers` |
| Hypervisor entitlement | no user action is documented: "Apple Silicon Macs include the hypervisor support that microsandbox uses." `msb` carries `com.apple.security.hypervisor` and `com.apple.security.cs.disable-library-validation` (`msb-entitlements.plist`) and is **ad-hoc signed** (`codesign --entitlements msb-entitlements.plist --force -s - build/msb`); no Developer ID, no notarization. The entitlement rides on `msb`, never on `yi`. The spike's first step confirms Gatekeeper runs the installed binary | <https://docs.microsandbox.dev/troubleshooting/macos.md>; `.github/workflows/release-macos.yml:129` at v0.7.3 |
| Root | the runtime lives under the user's `~/.microsandbox`; no step in the install or setup docs uses sudo. Each sandbox is an `msb machine` process running as the user. Only Linux needs a permission (read/write on `/dev/kvm`) | <https://docs.microsandbox.dev/configuration.md>, <https://docs.microsandbox.dev/sdk/setup.md>; `crates/runtime/lib/client/launch.rs:1-9`; `docs/troubleshooting/linux.mdx:7,53` at v0.7.3 |
| Guest architecture | Hypervisor.framework runs same-architecture guests, so the guest is `linux/arm64`. Every pull asks for the host's platform (`Platform::host_linux()`), there is no `--platform` flag, and there is no Rosetta or emulation; x86_64-on-ARM is an open proposal (#661, FEX-Emu). TB task images and yi's musl build are `x86_64` today (`Justfile:318`, `tbv4_sweep.sh:19`), so **TB images cannot run here**, and inner tasks need arm64 images | `crates/image/lib/platform.rs:53-64`, `registry/client.rs:726-757` at v0.7.3; <https://github.com/superradcompany/microsandbox/issues/661> |
| yi binary | `just package-musl <v> aarch64-unknown-linux-musl` takes the target as a parameter (`Justfile:318`) and `scripts/check_elf.py:15-19` knows `aarch64`, but no aarch64 musl build has ever been made: only `x86_64-unknown-linux-musl` is an installed rustup target on this host | `rustup target list --installed` |

## 3. Interfaces: which one evals/ drives

| interface | what it is | for evals/? |
|---|---|---|
| CLI `msb` | `run`, `create`, `exec` (`--timeout`, `--stream`, `-u`, `-w`, `-e`), `copy`/`cp`, `stop -f`, `rm -f`, `ls/ps/inspect --format json`, `metrics`, `logs`, `branch`, `snap create/restore`, `pull`, `image`, `doctor` | **yes.** stdlib `subprocess`, exactly as `evals/improve/round.py:137-141` drives `docker run` today. evals/ gains no dependency |
| Python SDK | `uv add microsandbox`; async `Sandbox.create(...)` with the same knobs | no. evals/ is stdlib-only |
| Rust SDK | `cargo add microsandbox`; `Sandbox::builder(..).cpus(..).memory(..).create().await` | no, see below |
| TS, Go, Ruby SDKs; MCP server; REST API for the hosted cloud | | no |

Sources: <https://docs.microsandbox.dev/cli/sandbox-commands.md>, <https://docs.microsandbox.dev/sdk/python/sandbox.md>, README.

**No Rust dependency.** yi's crates should never depend on `microsandbox`:

- Only the evals need VMs. The product does not, and a VMM in the product graph is attack
  surface and binary size that the dist-binary ratchet would have to absorb, for no user.
- The har-supply rule is "Does `std` do it? Does an already-present dependency do it?" A
  subprocess call to `msb` does it. The SDK crate has 54 direct dependencies (tokio, sea-orm with
  sqlx SQLite, reqwest, oci-client, rustls, rayon) and a lockfile closure of about 713 packages;
  it links Hypervisor.framework through `msb_krun`; and its `build.rs` downloads `msb` and
  `libkrunfw` from GitHub releases into `~/.microsandbox` at `cargo build` time
  (`sdk/rust/Cargo.toml`, `sdk/rust/build.rs:13-60`, `crates/filesystem/build.rs:62-67` at
  v0.7.3). A build script that reaches the network and writes `$HOME` is exactly the privileged
  build code har-supply exists to keep out.
- The Rust API itself broke in 0.7 (`LocalBackend::lazy()` and `build_lazy()` now return
  `Result`, <https://docs.microsandbox.dev/migrations/v0.7.md>). A beta's churn belongs behind a
  subprocess boundary, where a version bump is one runner file.
- A process that calls Hypervisor.framework itself carries the hypervisor entitlement. Embedding
  the VMM would move that signing requirement onto `yi`.

If yi ever runs its own tools in a microVM, that is a product decision for the sandbox
passthrough line (`docs/plans/2026-09-26-sandbox-passthrough.md`), not a side effect of the evals.

## 4. What a trial needs, and how each need maps

| need | microsandbox feature | design choice | source |
|---|---|---|---|
| yi binary in the VM | `msb copy`, a directory mount (`--mount-dir SRC:DST[:OPTIONS]`, options include `ro`), or a rootfs patch before boot (`--copy-dir`) | ✚ aarch64 musl build; mount its directory read-only at `/opt/yi` and put it first on `PATH`, as `round.py:139` mounts the binary read-only into docker | <https://docs.microsandbox.dev/cli/sandbox-commands.md>, <https://docs.microsandbox.dev/sandboxes/bootstrap.md> |
| seeded workspace | `git archive HEAD \| msb exec --stream W -- tar -x -C /workspace` is the documented PR-check pattern | stream `environment/app/` in as a tar, so the graded tree lives on the guest's ext4 (Linux case sensitivity, hard links, modes), not on a virtio-fs view of APFS | <https://docs.microsandbox.dev/examples/ci-cd/pr-checks.md> |
| session JSONL and events out, live | bind mount of a host directory: "changes inside the sandbox are reflected on the host" | mount the trial's host dir at `/logs/agent`, which is exactly where `yi_usage.run_command` writes (`yi_usage.py:17-20`). `watch.py`'s `trials()` then finds `<trial>/agent/yi.jsonl` (`watch.py:66-71`) while the trial runs, as it does for harbor | <https://docs.microsandbox.dev/sandboxes/volumes.md> |
| workspace out for scoring | `msb exec W -- tar -c -C /app .` to stdout; or `msb copy` | the verifier runs inside the VM after the agent exits, with `tests/` streamed in only then, so the agent never sees it; the tar of `/app` is kept beside the row for mining | same |
| egress only to openrouter.ai | `--net-default-egress deny --net-rule allow@openrouter.ai:tcp:443` plus gateway DNS; 0.7 requires interception for hostname rules | the secret already turns interception on (§1). agentd appends the CA to the system bundle and sets `SSL_CERT_FILE` to it; yi's TLS (`ureq` with `native-certs`, `Cargo.toml:62`) reads `SSL_CERT_FILE` on Linux, so it should trust the CA with no change. The runner must not set `SSL_CERT_FILE` itself, as the harbor adapter does to certifi's bundle (`agent.py:42-46`). Spike step 7 proves it | <https://docs.microsandbox.dev/networking/overview.md>, <https://docs.microsandbox.dev/changelog/2026-09-11.md>; `crates/agentd/lib/tls.rs:30-37,76-84` at v0.7.3 |
| API key as an env var, never on an argv | `--secret OPENROUTER_API_KEY@openrouter.ai`: the CLI reads the host variable at startup and rejects inline values; the guest sees `$MSB_OPENROUTER_API_KEY` | stronger than today: in `run.py` the key sits in yi's environment, and the bash tool inherits it (no `env_clear` in `crates/tools/src`); in the VM the agent cannot read the key at all. yi sends it as an `Authorization: Bearer` header (`crates/ai/src/request.rs:479`) to `https://openrouter.ai/api/v1` (`crates/ai/src/refresh.rs:36`), and headers are the default substitution location; yi checks no key prefix that a placeholder would fail | <https://docs.microsandbox.dev/sandboxes/secrets.md> |
| CPU and memory | `-c/--cpus`, `-m/--memory` (defaults 1 vCPU, 512 MiB) | 2 vCPU, 3 GiB per trial (the kernel venv loads pandas/numpy/scipy, `crates/kernel/src/bootstrap.rs:16-32`); 12 slots is 24 vCPU-equivalents on 16 cores, which is fine for I/O- and model-bound agents | <https://docs.microsandbox.dev/configuration.md> |
| 8-16 in parallel | no daemon; each sandbox is its own VM. Docs advise "a queue or global concurrency limit" per host | a stdlib `ThreadPoolExecutor(max_workers=slots)` in the runner | <https://docs.microsandbox.dev/examples/automation/parallel-batch-jobs.md> |
| timeouts and kill | `msb exec --timeout 5m`; `--max-duration 10m` and `--idle-timeout` on the sandbox; `msb stop -f`, `msb rm -f` | three nested bounds: yi's own `--deadline` (task `timeoutSec`), `exec --timeout` = deadline + 60 s, `--max-duration` = deadline + 180 s. `rm -f` in a `finally` | <https://docs.microsandbox.dev/cli/sandbox-commands.md>, <https://docs.microsandbox.dev/sandboxes/lifecycle.md> |
| base image | any OCI image; disk snapshots as warm baselines ("warm workers") | `python:3.12-slim` (what the proposer uses, `round.py:29`) plus git; bash is already in it. The kernel venv needs PyPI (`bootstrap.rs:537-560`), which the trial's egress rule forbids, so it is built once in a builder sandbox with PyPI egress and captured as a disk snapshot per venv slot (`bootstrap.rs:295-316`); every trial restores from it | <https://docs.microsandbox.dev/examples/sandboxing/warm-workers.md> |
| determinism | image layers pinned at first pull; one fresh writable layer per sandbox | pin the image by digest in the task format; record `msb --version`, the image digest and the snapshot id in the fingerprint | <https://docs.microsandbox.dev/images/overview.md> |
| startup cost at scale | the M1 guest-boot claim, and one harness: v0.4.5 on Linux x86 (GCP c3-standard-192-metal), median end-to-end CLI cold start 320 ms against Docker's 463 ms. No macOS numbers and no density or per-VM memory numbers are published | measured in the spike, end to end from the host | README; <https://microsandbox.dev/benchmarks>, <https://github.com/superradcompany/sandbox-bench> (`results/2026-05-12-c3-standard-192-metal-5way.json`) |

Two open mechanics the spike settles:

- Whether `msb snap restore` takes a network policy and secrets for the new sandbox. Its listed
  flags are name, `--forked`, `--disk-only`, volumes, ports, CPUs and memory
  (<https://docs.microsandbox.dev/cli/sandbox-commands.md>). If it does not, the fallback is to
  build the base image once with the host's docker, `msb load` it, and create every trial from
  that image with the trial's policy. The venv is then baked into the image.
- Which spelling of the default-deny flag the installed version accepts: the sandbox commands
  page writes `--net-default deny`, the networking overview and hardening pages
  `--net-default-egress deny`.

## 5. Snapshot and branch: a "fork at the failure point" eval

**It cannot fork a recorded session.** `msb branch` and full snapshots fork a VM that
microsandbox itself is running. Past TB trials ran in Docker and real sessions ran on macOS, so
no VM state exists for them.

**It can support forks for trials that run under microsandbox from now on,** with disk
snapshots, not memory branches:

1. A trial runs in a sandbox. `watch.py` already reads its `yi.jsonl` live. A deterministic
   trigger from `extract.py`'s signals (for example the first `spiral_cut`) names turn T.
2. The runner pauses the sandbox and takes a disk snapshot (`msb pause`, `msb snap create`);
   running and paused sandboxes both support disk-only snapshots (0.7.0 changelog).
3. For each arm, restore the snapshot, truncate the session file at turn T, and run
   `yi ask --continue` with that arm's binary. The harbor adapter already resumes this way
   (`agent.py:86`, `yi_usage.py:124`).

A memory branch buys nothing here. The candidate is a different binary, so it cannot continue
the base's process: it has to restart from the session file. Branching also drops yi's live
HTTPS stream ("live host connections are not captured"), and bind mounts must be supplied again
on restore.

What it would need that does not exist:

- ✚ `yi ask --continue` accepting a session truncated at an arbitrary turn, from a sibling
  binary. The kernel's Python state is gone at the fork, so the resumed run starts a fresh kernel.
- ✚ A fork task kind in the task format: a parent task, the trigger, and T.
- ✚ Gate statistics for forks. Forks share a prefix, so they pair on (task, T), one difference
  per fork point.

⏸ Deferred until the plain inner loop has produced verdicts. The prefix replay is the attractive
part (it spends the tokens only after the failure point), but it adds a resume contract to yi,
and the snapshot machinery is new: memory snapshots and branch shipped on 2026-09-16 (0.7.0),
and a disk-restore regression is open
([#1676](https://github.com/superradcompany/microsandbox/issues/1676)).

## 6. Maturity and risk

| signal | finding | source |
|---|---|---|
| Status | beta, by its own README | `README.md:113` at v0.7.3 |
| Age | repository created 2024-10-03; the current design is the 2026-04-03 "rewrite around embeddable SDK"; block-backed layers replaced the virtio-fs root on 2026-04-24; memory snapshots and branch shipped in 0.7.0 on 2026-09-16 | GitHub API `repos/superradcompany/microsandbox`; <https://docs.microsandbox.dev/changelog/2026-04-03.md>; <https://docs.microsandbox.dev/changelog/v0.7.0.md> |
| Cadence | 20 releases between 2026-07-01 and 2026-09-24; 0.7.0, 0.7.1 and 0.7.2 within two days; 56 crate versions since 2025-04-02 | GitHub API `releases`; <https://crates.io/crates/microsandbox> |
| Churn | 13 breaking changes from 0.6 to 0.7, including CLI flags, the secrets schema, the Rust API and stop semantics | <https://docs.microsandbox.dev/migrations/v0.7.md> |
| Bus factor | **two.** appcypher and toksdotdev wrote 216 and 162 of the 420 non-bot commits in the last 90 days (90%); the next person wrote 9. All time, the same two wrote 853 of 955 human commits | GitHub API `contributors`, `commits?since=2026-06-30` |
| Owner | superradcompany, which also sells a hosted cloud (billing, quotas, SSO in the same API) | <https://docs.microsandbox.dev/llms.txt> |
| Adoption | 8,453 stars, 455 forks; 31,096 crate downloads (14,328 recent) | GitHub API; crates.io |
| Open work | 63 open issues, 38 open PRs | GitHub search API, 2026-09-28 |
| libkrun | its own fork (`superradcompany/libkrun`, 5 stars), not upstream libkrun | `Cargo.toml:99-100` at v0.7.3 |
| Signing | ad-hoc only; not notarized | `release-macos.yml:129` |

Issues that bear on this use:

| issue | state | what | what it means here |
|---|---|---|---|
| [#1041](https://github.com/superradcompany/microsandbox/issues/1041) | closed 2026-06-27 | concurrent boots on macOS HVF (0.5.8, M2 Max): about 1 in 10 failed at 10 concurrent; a batch ran about 60× slower than one VM. The maintainer expected a 0.6.0 kernel fix to resolve it; the reporter never confirmed | exactly our 8-16 slot case, with no confirmed fix. Spike step 3 |
| [#1684](https://github.com/superradcompany/microsandbox/issues/1684) | open, 2026-09-27 | macOS arm64, 0.7.2: a long-lived SDK process fails every create after the 64th ("FOREIGN KEY constraint failed"); the workaround is one create per process | the CLI-per-create runner is that workaround; the spike still runs 100+ creates |
| [#1691](https://github.com/superradcompany/microsandbox/issues/1691) | open, 2026-09-28 | 0.7.3, macOS: false-positive placeholder detection intermittently blocks POSTs to an allowed host | every yi turn is a POST through that path. Spike step 7 sends 300 |
| [#1664](https://github.com/superradcompany/microsandbox/issues/1664) | closed 2026-09-25 | substitution dropped the connection when the body contained `%` | fixed in 0.7.4: pin ≥0.7.4, since prompts carry `%` |
| [#1596](https://github.com/superradcompany/microsandbox/issues/1596), [#1624](https://github.com/superradcompany/microsandbox/issues/1624) | closed | 0.7.0 was unusable on darwin-aarch64 (kernel library pairing, database migration, `self update`) until 0.7.1 | pin the version; upgrade on purpose |
| [#1687](https://github.com/superradcompany/microsandbox/issues/1687) | open | cancelling a create leaves an "ephemeral" sandbox behind, stopped | the runner labels its sandboxes by run and removes them all at exit |
| [#1642](https://github.com/superradcompany/microsandbox/issues/1642) | open | the local backend signals a stored runtime PID without checking it is still the runtime | a PID reused during a long campaign could be signalled |
| [#1558](https://github.com/superradcompany/microsandbox/issues/1558) | open | named-volume locks leak into child processes | the runner uses no named volumes |
| [#1334](https://github.com/superradcompany/microsandbox/issues/1334) | closed 2026-09-22 | orphaned `msb` processes at 100% CPU after the guest ring stayed full | spike step 9 counts processes after 100 runs |
| [#1683](https://github.com/superradcompany/microsandbox/issues/1683) | open | on a macOS host, virtio-fs answers `SEEK_DATA`/`SEEK_HOLE` swapped | the graded tree lives on the guest's ext4, not on a mount |
| [#1177](https://github.com/superradcompany/microsandbox/issues/1177) | closed | macOS bind mounts stripped the executable bit | spike step 4 runs yi from the mount; `msb copy` is the fallback |
| [#1226](https://github.com/superradcompany/microsandbox/issues/1226) | open | IPv6 blackholes on hosts without IPv6 egress | spike step 7 times yi's first connection |
| [#661](https://github.com/superradcompany/microsandbox/issues/661) | open | x86_64 images on ARM hosts (FEX-Emu), not implemented | no TB image runs on this Mac |
| [#1676](https://github.com/superradcompany/microsandbox/issues/1676) | open | disk restore loses the systemd init | the base snapshot uses no systemd |

Most of these closed within days, so the project fixes fast. But two people carry it; macOS
parallel boots have a failure history with no confirmed fix; and the path every yi request takes
(secret substitution under interception) has an open false positive on macOS. That makes the
spike's parallel and secret steps the deciding ones.

## 7. The alternative: temp workspaces and yi's own Seatbelt

What `run.py` does today: a temp copy of the task repo, a temp HOME per process
(`run.py:232-242`), `yi ask --yolo --here` on the host (`run.py:133-148`).

One correction to the premise: **under `--yolo`, Seatbelt does not contain the bash tool.**
`crates/permission/src/decide.rs:218-220` returns `Allow` in yolo mode. Seatbelt applies to a
`Contain` decision (`crates/runtime/src/gate.rs:138`), which only auto mode's safety verdict
produces (`decide.rs:221,271`), and to the kernel's exec profile
(`crates/runtime/src/wiring.rs:163-167`). So for the bash tool, the plain runner's isolation is
a temp directory and nothing more. Containing it would need auto mode or a new eval mode, and
Seatbelt's profile has no network rule, so contained commands get no egress at all
(`crates/tools/src/sandbox.rs:9-10`).

| axis | plain runner (`run.py`) | microsandbox runner |
|---|---|---|
| Isolation of the host | none under `--yolo`: the agent runs as the owner, on the owner's disk; the catastrophic denylist is the only wall (`crates/permission/src/decide.rs:123`) | a VM: own kernel, five virtio devices, rootfs copy-on-write |
| API key exposure | in yi's environment, inherited by the bash tool | never in the guest; a placeholder substituted at the host proxy |
| Egress control | none | deny by default, openrouter.ai only |
| Speed per trial | zero boot | claimed <100 ms guest boot; plus copy-in and copy-out; measured in the spike |
| Fidelity to TB's Linux images | macOS userland (BSD tools, APFS, case-insensitive), yi's macOS code paths | Linux arm64 userland, ext4, root. TB images are x86_64 and cannot run on this Mac at all (§2), so it is Linux fidelity, not image fidelity |
| Fidelity to real yi sessions | high: most of the owner's sessions run on this Mac | lower for macOS-specific failures |
| Parallelism | shared host state: the caller's uv cache (`run.py:400-403`), shared kernel venvs, ports, the real HOME that some paths still touch | per-VM HOME, venv, ports and /tmp |
| Stability | no new moving parts | a beta runtime with a macOS parallel-boot history (#1041) and open bugs on our path (#1684, #1691): see §6 |
| Effort | the task format and a partial-score read | the same, plus `msb_runner.py`, an aarch64 musl build, the base snapshot, and an msb stop in `watch.py` |
| Host weight | nothing | one small base image plus a venv snapshot, shared copy-on-write; up to 3 GiB RAM per running VM (the cap we set) |

Scored on the owner's rubric: both fix complaint 1's TB-specific causes and complaint 5.
Neither affects 2 and 3. For 4, both parallelize, and microsandbox isolates the slots. The deciding axis
is isolation: 150 tasks × k × two arms per candidate is thousands of unattended `--yolo` runs a
month on the owner's workstation, with the key in reach of every bash call.

## 8. The runner, as a composition of existing pieces

```
evals/drivers/msb_runner.py <overrides.json> <task>...        (the protocol of levers.py:14-16)
  caps     trials.py caps --run-id $EVAL_RUN_ID --tasks N      ✓ (per-trial price is a parameter ✚)
  watch    watch.py --runs <runs>/<run-id> --engine msb ...    ✓ caps, ✚ msb stop
  per trial, N slots in parallel:
    msb create <image@digest | base snapshot> --name <trial> --label run=<run-id>
        --cpus 2 --memory 3G --max-duration <deadline+180>s
        --net-default-egress deny --net-rule allow@openrouter.ai:tcp:443
        --secret OPENROUTER_API_KEY@openrouter.ai
        --mount-dir <aarch64 dist dir>:/opt/yi:ro --mount-dir <runs>/<trial>/agent:/logs/agent
    tar c environment/app | msb exec --stream <trial> -- tar -x -C /app
    msb exec --timeout <deadline+60>s -w /app <trial> -- sh -c "<yi_usage.run_command(...)>"
    tar c tests | msb exec --stream ... ; msb exec ... bash /tests/test.sh   (APP, TESTS, LOGS as run.py:90-96)
    msb exec <trial> -- tar c -C /logs/verifier . | tar x -C <runs>/<trial>/verifier
    write <runs>/<trial>/row.json ; finally msb rm -f <trial>
  exit     msb rm -f --label run=<run-id>   (sweeps what a cancelled create left, #1687)
  rows     trials.py rows <runs>/<run-id> --run-id --arm       ✓ (axes.run_context reads partials ✚)
  gate     levers.py per-task differences, two roads            ✓
```

| piece | status | where |
|---|---|---|
| Runner protocol `<runner> <overrides.json> <task>...`, rows on stdout, `YI_LEVERS` in the env | ✓ | `evals/levers.py:14-16`; `tbv4_sweep.sh:6-10` |
| The in-guest command: `--eval`, `--yolo --here`, `--deadline`, the `message_update` filter, `/logs/agent` layout | ✓ reused verbatim | `yi_usage.run_command`, `yi_usage.py:112-145` |
| Trial HOME config (telemetry, `EVAL_ROUTING`) | ✓ | `yi_usage.eval_config`, `yi_usage.py:47-61` |
| Levers file into the guest | ✓ path | `REMOTE_LEVERS_PATH`, `yi_usage.py:20`; `agent.py:122-128` |
| Verifier contract (`tests/test.sh` with `APP`, `TESTS`, `LOGS`) | ✓ | `run.py:89-96` |
| Graded score from `trace_results.json` `partial_score` | ✓ harbor rows; ✚ run rows | `axes.py:81-88`; `axes.run_context`, `axes.py:109-112`, learns `partialScore` and `censored` |
| Trial store, stage and week caps | ✓ | `evals/drivers/trials.py:72-99` |
| Per-trial $ and turn caps, `censored` marker, free-space stop | ✓ caps; ✚ an msb stop beside `stop_containers` | `watch.py:66-83` |
| Rerun a trial whose environment never started | ✓ rule; ✚ the msb runner applies it (no session file = rerun once) | `tbv4_sweep.sh:79-94`, `trials.py:54-66` |
| Paired gate over per-task differences | ✓ | `levers.py`; design §6.3 |
| **msb runner** | ✚ | `evals/drivers/msb_runner.py` (one file, stdlib, `subprocess` + `concurrent.futures`) |
| **Inner-loop tasks** | ✚ data | `evals/fixtures/inner/<id>/` in run.py's harbor layout |
| **Offline check** | ✚ | `selftest.py::check_msb_runner`: builds the argv for a fixture task with `msb` absent and pins that the key never appears on it, the egress rule is present, and the deadlines nest |

**The task format** is the harbor layout `run.py` already runs (`run.py:116-118`), so one task
runs on both runners:

```
evals/fixtures/inner/<id>/
  task.json          {id, class, timeoutSec ≤ 300, image, source: "tb:<trial>" | "session:<id>",
                      signal: "<extract.py signal it targets>", approved: "<owner, date>"}
  instruction.md     the prompt
  environment/app/   the seed tree
  tests/test.sh      writes $LOGS/trace_results.json {"partial_score": x} and exits 0/1
```

Mined tasks are original. The drafter reads failure modes and trial rows, never TB task files,
which stay out of git (design §12). `approved` is written only by the owner.

**New files:** `evals/drivers/msb_runner.py` and the `evals/fixtures/inner/` tree. **Extended:**
`axes.py` (`run_context` partials), `watch.py` (msb stop), `run.py` (the partial score from
`trace_results.json`, so the plain runner grades the same tasks), `selftest.py` (one check),
`trials.py` (per-trial price as a parameter). Nothing in `crates/`.

## 9. Build order

| stage | builds | demo | gate | cost |
|---|---|---|---|---|
| **0: spike** (owner approves the installs) | nothing committed. Install `msb` by Homebrew, pinned at ≥0.7.4 (the `%` fix, #1664); `rustup target add aarch64-unknown-linux-musl`; one aarch64 musl build | the numbers below, in a ledger note | the kill criteria in §10 | $0, or $0.01 with the optional TLS check |
| 1 | the task format; `axes.run_context` and `run.py` read partials; 10 hand-made inner tasks | `run.py --task …` grades them | `selftest.py` pins the format and the partial read | under $1 |
| 2 | mining: the LLM drafts tasks from `extract.py` signals and TB rows; the owner approves each | 50, then 150 approved tasks | each task's verifier is red on its seed and green on a reference fix (the yi-refute rule) | drafting tokens only |
| 3 | `msb_runner.py`, the base snapshot, the `watch.py` stop, `check_msb_runner` | one faux run of all tasks; then one paid run at k=1 | 0 environment losses on the faux run | ~$5-8 per 150-task arm, assuming a mined trial costs 3-4x a fixture trial ($0.007-0.013 in rows 0054 and 0056) |
| 4 | A/A: plain against msb, and msb against msb, same binary | the task-level difference table | msb loses no more trials than plain; its A/A spread is no wider | two arms |
| 5 | msb becomes the inner loop's default runner | | | |
| ⏸ | the fork-at-failure task kind (§5) | | | |

**The spike, step by step.** Every step records wall time from the host, `msb --version`, and
`~/.microsandbox` disk use before and after.

1. `msb doctor`, and that Gatekeeper runs the ad-hoc-signed binary. Pull `python:3.12-slim`
   (arm64): time and bytes.
2. Cold boot: the first `msb run --no-net python:3.12-slim -- true` after the pull. Warm boot:
   median and p95 of 20 sequential runs.
3. Parallel start: 20 concurrent `msb create` + `msb exec -- true`, repeated 5 times (100
   starts, past #1684's 64-create mark). Time to all ready, failures, host RSS per VM, open file
   descriptors. This is #1041's case, which has no confirmed fix on macOS.
4. `yi --version` in the guest from the aarch64 musl build, mounted read-only (the exec bit
   survives the mount, #1177).
5. The base snapshot: a builder sandbox with PyPI egress runs `yi doctor --fix --json`; capture
   it; restore 10 workers from it; confirm the kernel is ready in each with no network.
6. One faux trial: `yi ask --model faux/faux-1 --json` through `yi_usage.run_command` in a
   restored worker, sessions landing on the host through the `/logs/agent` mount.
7. The network allowlist: `curl https://openrouter.ai/api/v1/models` succeeds; `example.com`,
   `1.1.1.1`, `pypi.org`, `169.254.169.254` and `host.microsandbox.internal` fail. The secret:
   `env` in the guest shows only the placeholder; `curl -H "Authorization: Bearer
   $OPENROUTER_API_KEY" https://openrouter.ai/api/v1/key` returns the key's metadata (a free
   endpoint); the same header sent to a second allowed host is blocked. `msb exec W -- env` shows
   the `SSL_CERT_FILE` agentd set. Then 300 POSTs to `/api/v1/chat/completions` naming a model
   that does not exist, with bodies carrying `%`, non-ASCII text and long JSON: OpenRouter
   answers 400 and should bill nothing, and any proxy block is #1691. Time to first byte of the
   first connection (IPv6 blackholes, #1226). Optional, $0.01 cap: one real one-turn `yi ask`
   to prove yi's own TLS stack trusts the interception CA.
8. Extraction: stream a 50 MB tree out with `tar`; checksums match the guest's `sha256sum`.
   The host sees `yi.jsonl` grow live through the mount.
9. Kill: `exec --timeout 5s -- sleep 60` returns on time; `--max-duration 30s` stops the VM;
   `rm -f` mid-run leaves no `msb` process, no `msb ls` entry, and no disk growth after 100 runs.
10. Determinism: the faux trial 10 times gives identical session files, modulo ids and
    timestamps.

## 10. Risks and kill criteria

| risk | likelihood | what we'd see | kill or mitigate |
|---|---|---|---|
| Beta churn breaks the runner on upgrade | high: 13 breaking changes in 0.7, flag names differ between doc pages | a runner that fails after `brew upgrade` | pin the version; upgrade on purpose, rerun the spike's steps 3, 7 and 9 |
| Parallel starts fail or leak state | medium: [#1041](https://github.com/superradcompany/microsandbox/issues/1041) failed 1 in 10 at 10 concurrent on macOS, with no confirmed fix; [#1684](https://github.com/superradcompany/microsandbox/issues/1684), [#1687](https://github.com/superradcompany/microsandbox/issues/1687) are open | lost trials; stale sandboxes; `~/.microsandbox` growth | **kill** if the spike loses any of 100 starts, or a start takes >30 s |
| Strict hostnames need interception, and yi's TLS stack does not trust the CA | low: agentd sets `SSL_CERT_FILE` to the bundle holding the CA | yi's first request fails with an unknown issuer | set `SSL_CERT_FILE` in the in-guest command; **kill** only if it needs a change in `crates/` |
| Secret substitution blocks a real request | medium: [#1691](https://github.com/superradcompany/microsandbox/issues/1691) is open on 0.7.3 macOS | censored or errored trials with a proxy error, not a model error | **kill the secret path** if any of the spike's 300 POSTs is blocked; the fallback streams the key over stdin into a mode-600 file in the guest (never on an argv) and keeps the egress rule, which is what the harbor containers do today, with less exposure |
| Egress escapes the allowlist | low (DNS pinning, SNI checks) | a denied host answers | **kill** |
| The key is readable in the guest | low | the real value in `env`, `/proc` or a request to another host | **kill** |
| The inner loop stops predicting TB (arm64 Linux vs x86_64 TB images; macOS for real sessions) | medium | inner-loop verdicts that validation reverses | the gate already demands a TB validation pass (design §6.3); track the reversal rate |
| Boot is fast but the whole trial setup is slow | medium | >2 s median to a ready shell at 16 parallel | **kill** above 5 s p95: the plain runner is then as good |
| Bus factor two; one company's hosted cloud pulls the local runtime's priorities | medium | releases stop, or local-only features (full snapshots, branch) stall | the runner uses only create, exec, copy, rm and a disk snapshot; a pinned Apache-2.0 release keeps working; the protocol drops back to `run.py` |
| Hypervisor or host instability on the owner's workstation | low | panics, stuck VMs, a Mac that needs a reboot | **kill** on the first host-level fault |

**Kill for the whole idea:** the spike misses any kill criterion, or stage 4 shows msb losing
more trials than plain, or two upgrades in a month each break the runner. Then the inner loop
stays on `run.py`, and nothing else changes, because the runner protocol hides the choice.

## 11. Open questions for the owner

1. **Approve the spike's installs?** `brew install superradcompany/tap/microsandbox` (pinned)
   and `rustup target add aarch64-unknown-linux-musl`. Nothing else is installed.
2. **Isolation or fidelity to your Mac?** Many of the real sessions the tasks are mined from ran
   on macOS. Should a task mined from a macOS session run on the plain runner, and a task mined
   from TB run in a VM? The format allows both; the gate would then pair within each class.
3. **One image for all inner tasks?** A task that needs a toolchain outside the base image
   (cargo, bun) either gets a second small image or is out of the inner loop. The recommendation
   is a closed list of at most three images, pinned by digest.
4. **Per-trial cap.** The watcher's $1 and 180-turn caps fit 45-minute TB trials. A ≤5 minute
   inner task probably wants $0.25 and 60 turns.
5. **Weekly budget.** A 150-task paired stage at k=1 is about 300 trials. At $0.03-0.05 per
   mined trial (fixture trials run $0.007-0.013), that is $9-15 per candidate against the $25/$30 week
   of §18. Does the inner loop get its own line?
6. **The fork-at-failure eval (§5):** worth a resume contract in yi later, or not at all?

## 12. Owner decision (2026-09-28)

Asked whether to approve the spike, which installs microsandbox and runs it through the `msb` CLI, the owner answered:

> "audit, copy the patterns, own microsandbox code directly in yi crate, port the highest quality most important code and cut the bloat and make our own yi flavored version HAR compliant"

So the subprocess-to-`msb` runner of §8 and the spike of §9 are superseded. §3's case against a Rust dependency on the `microsandbox` crate still holds: the port owns the code instead of depending on it. The findings in §1-§7 are the audit's starting map. The open forks the port must decide are the hypervisor backend (libkrun as `msb_krun`, or Apple's Virtualization.framework on macOS), where the hypervisor entitlement lives (a separate signed helper binary, or `yi` itself), and the port's scope. The port is a separate proposal.
