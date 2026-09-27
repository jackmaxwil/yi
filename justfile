set positional-arguments

default: check

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# nextest when it is on PATH, cargo when it is not, so a fresh clone still
# runs the suite. `cargo test` runs the 101 test binaries one after another
# and five of them are 96 % of the wall clock; nextest pools every test from
# every binary, and .config/nextest.toml's slow-timeout kills a hang in 60s
# instead of letting it hold a runner. nextest cannot run doctests, so those
# stay a second cargo invocation rather than quietly leaving the gate.
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-nextest >/dev/null; then
      cargo nextest run --workspace
      cargo test --workspace --doc
    else
      cargo test --workspace
    fi
    # A workspace build unifies proptest's regex-syntax, which links every Unicode table, so the
    # table guards see only what the binary ships when yi-cli's graph resolves the features alone.
    cargo test -p yi-cli -p yi-tui -p yi-tools --test integration --test tools linked_tables

# The lanes `check` runs, named so CI can run them as separate jobs.
lint: fmt-check clippy

# The one line for a dev loop: rebuild, stop the daemon the last build left running,
# open the workspace. ctrl+c twice inside yi does the stop for you; ⌥q keeps it running.
dev:
    cargo build -p yi-cli
    pkill -f 'yi serve' || true
    ./target/debug/yi

build-dist:
    scripts/build_dist.sh

# No build-dist dependency: the aggregator is the one place that knows whether
# binary_size and startup will run at all, so it owns the LTO build they read
# (D91). CI skips both, and paid 2-3 min a job for a binary nothing measured.
guardrails:
    bash scripts/guardrails/check_guardrails.sh

# The build-free subset, which is what keeps pre-commit under two seconds.
guardrails-fast:
    bash scripts/guardrails/check_guardrails.sh --fast

# Serial on purpose: beside the dist build, nextest's first exec of 134 fresh test binaries
# queued behind Gatekeeper's per-binary scan, and the gate took 116-302s where serial took 135-151s.
check: lint guardrails test

# --- Local CI: three tiers, run by the hooks in scripts/hooks ----------------

# Every worktree with its branch and dirt, and the stash they all share.
worktrees:
    #!/usr/bin/env bash
    set -euo pipefail
    git worktree list --porcelain | sed -n 's/^worktree //p' | while read -r path; do
      branch=$(git -C "$path" symbolic-ref --quiet --short HEAD || echo "(detached)")
      count=$(git -C "$path" status --porcelain | wc -l | tr -d ' ')
      [ "$count" = 0 ] && state=clean || state="$count dirty"
      printf '%-34s %-12s %s\n' "$branch" "$state" "$path"
    done
    # The stash stack is one stack for every worktree, which is why `git stash
    # pop` here can take a concurrent session's work.
    echo "stash: $(git stash list | wc -l | tr -d ' ') entries, shared by all of the above"

# Point git at the versioned hooks (per-repo, shared by every worktree).
install-hooks:
    git config core.hooksPath scripts/hooks
    @echo "hooks installed: scripts/hooks"

# Tier 1, every commit: source-only checks, no build and no suite. The three share nothing
# but cargo's lock, so they overlap: 6-7s on an unchanged tree became 1.7s.
[parallel]
precommit: fmt-check clippy guardrails-fast

# Tier 2, every push: the full gate, and nothing the gate itself regenerated.
prepush: (lane "check")

# One lane of the gate, carrying the invariant. `lint`, `guardrails` and
# `test` do not depend on each other, so CI runs them as parallel jobs and
# pays the longest rather than the sum; the invariant has to ride the lane
# rather than the whole gate, or a split run stops asserting it.
lane name:
    #!/usr/bin/env bash
    set -euo pipefail
    # Not "is the tree clean" — a push with unrelated WIP is normal. The
    # invariant is that the gate wrote nothing: the ratchet baselines are
    # generated files, and one left behind fails on the next machine.
    before=$(git status --porcelain)
    just {{name}}
    if [ "$before" != "$(git status --porcelain)" ]; then
      echo "the gate regenerated files — commit them:"
      git status --porcelain
      exit 1
    fi

# Tier 3, every version tag: check the tag against Cargo.toml, then the artifact.
prerelease version:
    #!/usr/bin/env bash
    set -euo pipefail
    declared=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    if [ "{{version}}" != "$declared" ]; then
      echo "tag {{version}} disagrees with Cargo.toml $declared"
      exit 1
    fi
    just package "{{version}}"
    echo "prerelease {{version}}: ok"

# Real-binary journeys over the faux model: keyless, offline, and slow enough
# (a process tree or a kernel boot per assertion) to stay out of `just check`,
# where #[ignore] keeps them.
journeys:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test --workspace -- --ignored
    # The true-terminal journey has no cargo home: no test can hand the binary
    # a pty that answers its cursor-position query, so the harness runs here.
    cargo build -p yi-cli
    home="$PWD/target/journeys"
    rm -rf "$home"
    mkdir -p "$home"
    # Incident: a drive run read the developer's own ~/.yi/config.json, so a
    # `keys` entry there decided whether it passed.
    HOME="$home" python3 scripts/tui_pty.py --send-quit --expect '┃' \
      --expect 'faux:' -- tui --model faux/faux-1 --session-dir "$home/sessions" ping

# The console counterpart of tui-proof: a faux daemon on a scratch socket drives the
# real workspace shell headless and agg renders the cast.
console-proof script out="target/proof":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{out}}" in /*|*..*) echo "out must be a relative path in the repo"; exit 1 ;; esac
    out="$PWD/{{out}}"
    rm -rf "$out"
    mkdir -p "$out/frames" "$out/home/.yi"
    printf '{"kernel":{"prewarm":false}}' > "$out/home/.yi/config.json"
    cargo build -q -p yi-cli
    # A unix socket path is capped near 100 bytes, so it lives under /tmp, not the repo.
    sock="$(mktemp -d /tmp/yi-proof.XXXXXX)/d.sock"
    HOME="$out/home" target/debug/yi serve --socket "$sock" --model faux/faux-1 \
      --session-dir "$out/sessions" >"$out/serve.log" 2>&1 &
    pid=$!
    trap 'kill $pid 2>/dev/null || true; rm -rf "$(dirname "$sock")"' EXIT
    for _ in $(seq 1 60); do [ -S "$sock" ] && break; sleep 0.05; done
    HOME="$out/home" target/debug/yi console --headless --socket "$sock" \
      --keys "{{script}}" --frames "$out/frames" --record "$out/run.cast"
    rendered() { [ -s "$1" ] || { echo "empty render: $1"; exit 1; }; }
    if command -v agg >/dev/null; then
      agg --theme monokai --font-size 16 --idle-time-limit 1 "$out/run.cast" "$out/run.gif"
      rendered "$out/run.gif"
      echo "motion: $out/run.gif"
    else
      echo "no agg on PATH — cast only (brew install agg)"
    fi
    echo "cast: $out/run.cast"

# The rail's avatars and the pane's orb are kitty images: a real console under a pty
# claiming xterm-kitty against a faux daemon must place at least one avatar (ids 8000+)
# per rail row and one orb (ids 7800+) for the chat pane.
console-pty out="target/proof":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{out}}" in /*|*..*) echo "out must be a relative path in the repo"; exit 1 ;; esac
    out="$PWD/{{out}}"
    rm -rf "$out"
    mkdir -p "$out/home/.yi"
    printf '{"kernel":{"prewarm":false}}' > "$out/home/.yi/config.json"
    cargo build -q -p yi-cli
    sock="$(mktemp -d /tmp/yi-pty.XXXXXX)/d.sock"
    HOME="$out/home" target/debug/yi serve --socket "$sock" --model faux/faux-1 \
      --session-dir "$out/sessions" >"$out/serve.log" 2>&1 &
    pid=$!
    trap 'kill $pid 2>/dev/null || true; rm -rf "$(dirname "$sock")"' EXIT
    for _ in $(seq 1 60); do [ -S "$sock" ] && break; sleep 0.05; done
    HOME="$out/home" python3 scripts/tui_pty.py --term xterm-kitty --seconds 6 \
      --raw "$out/pty.raw" --send-quit -- console --socket "$sock" --cwd "$PWD" >"$out/pty.txt"
    avatars=$(grep -ao $'\x1b_Ga=p,i=80[0-9][0-9],' "$out/pty.raw" | sort -u | wc -l | tr -d ' ')
    orbs=$(grep -ao $'\x1b_Ga=p,i=78[0-9][0-9],' "$out/pty.raw" | sort -u | wc -l | tr -d ' ')
    echo "avatars placed: $avatars   orbs placed: $orbs"
    [ "$avatars" -ge 1 ] && [ "$orbs" -ge 1 ]

# A drive script rendered for people: the motion and still GIFs a UI change
# attaches to its PR, so a reviewer can judge how it looks. `agg` is dev-only
# (brew install agg) and never enters the binary.
tui-proof script out="target/proof":
    #!/usr/bin/env bash
    set -euo pipefail
    # Incident: `out` is a caller argument that is `rm -rf`'d and was also
    # spliced after $PWD for the HOME isolation, so an absolute value deleted
    # a path outside the repo and isolated a directory it never wrote to.
    case "{{out}}" in /*|*..*) echo "out must be a relative path in the repo"; exit 1 ;; esac
    out="$PWD/{{out}}"
    rm -rf "$out"
    mkdir -p "$out/frames"
    # Same isolation as `journeys`: a `keys` entry in the developer's own
    # ~/.yi/config.json must not decide what the proof shows.
    home="$out/home"
    mkdir -p "$home"
    HOME="$home" cargo run -q -p yi-cli -- tui --headless --model faux/faux-1 \
      --session-dir "$out/sessions" --keys "{{script}}" \
      --frames "$out/frames" --record "$out/run.cast" \
      --snap "$out/still.cast" --deadline 300
    # Incident: a renderer can exit 0 and write an empty file, so a proof that
    # rendered nothing printed a path and looked like it worked.
    rendered() { [ -s "$1" ] || { echo "empty render: $1"; exit 1; }; }
    # One renderer for both artifacts: agg is a terminal emulator, so the
    # still and the motion agree cell for cell. Theme and size are pinned so
    # proofs from different machines look alike.
    if command -v agg >/dev/null; then
      agg --theme monokai --font-size 16 --idle-time-limit 1 \
        "$out/run.cast" "$out/run.gif"
      rendered "$out/run.gif"
      agg --theme monokai --font-size 16 "$out/still.cast" "$out/still.gif"
      rendered "$out/still.gif"
      echo "motion: $out/run.gif"
      echo "still:  $out/still.gif"
    else
      echo "no agg on PATH — casts only (brew install agg)"
    fi
    echo "cast: $out/run.cast"

# Tier 4, after a merge: the journeys. The dist-profile suite left this recipe for
# `dist-suite` (#304): a release build of the workspace and its tests that no cache
# warms was seven of the postmerge job's eight minutes, and it gates nothing.
postmerge: journeys

# The suite against the profile that ships, unwind forced, which nothing else
# covers because the shipping profile aborts. Nightly in CI (`dist-suite.yml`).
dist-suite:
    CARGO_PROFILE_DIST_PANIC=unwind cargo test --workspace --profile dist

# Tier 4 sibling: the task-eval runner over its fixtures, faux only. Offline and
# keyless, but it wants a built binary, so it stays out of `just check`.
postmerge-evals:
    cargo build -p yi-cli
    python3 evals/run.py --dry --binary target/debug/yi --model faux/faux-1
    python3 evals/surface.py --dry --binary target/debug/yi --model faux/faux-1
    python3 evals/judge_replay.py all --dry --model faux/faux-1

# Prefill the PR narrative's counted sections from the diff against main.
pr-body:
    python3 scripts/pr_body.py

# Finishing a branch, as verbs that check first (the yi-forge skill is the procedure):
# ratchet the baselines alone, commit with a judged subject, push through the lane,
# open the PR with the title job's own judge, read the gate, merge with the reason.
ratchet *args:
    python3 scripts/forge_pr.py ratchet "$@"

# One ADR from its decision-log row, plus the index line.
adr number:
    python3 scripts/adr.py {{number}}

commit subject *args:
    python3 scripts/forge_pr.py commit "$@"

push:
    python3 scripts/forge_pr.py push

pr *args:
    python3 scripts/forge_pr.py pr "$@"

# The whole landing: merge main (baselines merge themselves), reprice the growth memo from
# main's baseline, re-ratchet, render missing ADRs, open, wait, merge.
land title *args:
    python3 scripts/forge_pr.py land "$@"

# Upload an already-built, signed release to Forgejo (release-scoped token).
publish version:
    #!/usr/bin/env bash
    set -euo pipefail
    : "${FORGEJO_URL:?set FORGEJO_URL (e.g. https://code.example.org)}"
    : "${FORGEJO_REPO:?set FORGEJO_REPO (e.g. jack/yi)}"
    : "${FORGEJO_TOKEN:?set FORGEJO_TOKEN (a release-scoped token)}"
    api="$FORGEJO_URL/api/v1/repos/$FORGEJO_REPO/releases"
    auth="Authorization: token $FORGEJO_TOKEN"
    assets=(target/package/yi-{{version}}-*.tar.gz*)
    [ -e "${assets[0]}" ] || { echo "nothing built for {{version}} -- run: just prerelease {{version}}"; exit 1; }
    body=$(printf '{"tag_name":"v%s","name":"v%s"}' '{{version}}' '{{version}}')
    # python3, not sed: the response carries several "id" keys -- the author's
    # among them -- and a greedy match takes the last one rather than the
    # release's. python3 is already a hard requirement for the guardrails.
    id=$(curl -fsS -X POST "$api" -H "$auth" -H 'Content-Type: application/json' -d "$body" \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
    for asset in "${assets[@]}"; do
      curl -fsS -X POST "$api/$id/assets?name=$(basename "$asset")" -H "$auth" \
        -F "attachment=@$asset" > /dev/null
      echo "uploaded $(basename "$asset")"
    done

# One release tarball for the host, signed, then the smoke test on it.
package version target=`rustc -vV | sed -n 's|host: ||p'`:
    scripts/build_dist.sh --target {{target}}
    scripts/package.sh {{version}} {{target}}
    scripts/smoke.sh target/package/yi-{{version}}-{{target}}.tar.gz

# Cross-build + package a musl target via zig (no musl-gcc/cross needed).
# Fails closed by name when the rustup target or zig is missing (O1): a
# missing toolchain must never be reported as a packaged artifact. Cannot
# smoke-test the result -- the cross binary does not run on this host -- so
# that step is named skipped rather than silently omitted; the container's
# `yi --version` preflight (evals/README) is the real smoke.
package-musl version target='x86_64-unknown-linux-musl':
    #!/usr/bin/env bash
    set -euo pipefail
    target='{{target}}'
    rustup target list --installed | grep -qx "$target" || {
      echo "package-musl: rustup target '$target' not installed -- run: rustup target add $target"
      exit 1
    }
    command -v zig >/dev/null || {
      echo "package-musl: zig not found -- install zig, or use musl-cross gcc / cargo-zigbuild instead"
      exit 1
    }
    cargo_var=CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER
    cc_var=CC_$(echo "$target" | tr '-' '_')
    ar_var=AR_$(echo "$target" | tr '-' '_')
    export "$cargo_var=$(pwd)/scripts/zigcc.sh"
    export "$cc_var=$(pwd)/scripts/zigcc.sh"
    export "$ar_var=$(pwd)/scripts/zigar.sh"
    export ZIG_TARGET="${target%-unknown*}-${target##*-unknown-}"
    # Incident: rustc's self-contained musl crt (rcrt1.o) and zig's crt1.o both
    # define _start_c; zig supplies the crt, so rustc must not.
    export RUSTFLAGS="${RUSTFLAGS:-} -C link-self-contained=no"
    scripts/build_dist.sh --target "$target"
    python3 scripts/check_elf.py "target/$target/dist/yi" "$target"
    scripts/package.sh {{version}} "$target"
    echo "package-musl: smoke.sh NOT run on this host (cross binary can't execute on darwin);"
    echo "package-musl: the container's 'yi --version' preflight (evals/README) is the real smoke."

# Catalog skills (§7.8) install into the global root; the fragments an
# extension attaches are compiled in.
install-skills:
    mkdir -p ~/.yi/skills/yi
    rsync -a --delete skills/yi/ ~/.yi/skills/yi/
    @for stale in caveman ponytail superpowers diagram-design; do [ -d ~/.yi/skills/$stale ] && echo "stale bundle, remove by hand: ~/.yi/skills/$stale" || true; done
