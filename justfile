default: check

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

# Per-test timeouts and a slow-test report; `test` stays on cargo, no install.
test-nextest:
    cargo nextest run --workspace

build-dist:
    cargo build --profile dist -p yi-cli

# Depends on build-dist: binary_size and startup read target/dist/yi, and a
# gate measuring whatever stale binary is lying around measures nothing.
guardrails: build-dist
    bash scripts/guardrails/check_guardrails.sh

# The build-free subset, which is what keeps pre-commit under two seconds.
guardrails-fast:
    bash scripts/guardrails/check_guardrails.sh --fast

check: fmt-check clippy guardrails test

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

# Tier 1, every commit: source-only checks, no build and no suite.
precommit: fmt-check clippy guardrails-fast

# Tier 2, every push: the full gate, and nothing the gate itself regenerated.
prepush:
    #!/usr/bin/env bash
    set -euo pipefail
    # Not "is the tree clean" — a push with unrelated WIP is normal. The
    # invariant is that the gate wrote nothing: the ratchet baselines are
    # generated files, and one left behind fails on the next machine.
    before=$(git status --porcelain)
    just check
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

# Tier 4, after a merge: the suite against the profile that ships, unwind forced.
postmerge:
    CARGO_PROFILE_DIST_PANIC=unwind cargo test --workspace --profile dist

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
    cargo build --profile dist -p yi-cli --target {{target}}
    scripts/package.sh {{version}} {{target}}
    scripts/smoke.sh target/package/yi-{{version}}-{{target}}.tar.gz

# Catalog skills (§14.1) install into the global root; the fragments an
# extension attaches are compiled in.
install-skills:
    mkdir -p ~/.yi/skills
    cp -R skills/. ~/.yi/skills/
