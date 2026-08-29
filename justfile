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

# Depends on build-dist: binary_size and startup both read target/dist/yi, and
# a gate that measures whatever stale binary happens to be there measures noise.
guardrails: build-dist
    bash scripts/guardrails/check_guardrails.sh

# The build-free subset, which is what keeps pre-commit under two seconds.
guardrails-fast:
    bash scripts/guardrails/check_guardrails.sh --fast

check: fmt-check clippy guardrails test

# --- Local CI: three tiers, run by the hooks in scripts/hooks ----------------

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

# One release tarball for the host, then the smoke test on it.
package version target=`rustc -vV | sed -n 's|host: ||p'`:
    cargo build --profile dist -p yi-cli --target {{target}}
    scripts/package.sh {{version}} {{target}}
    scripts/smoke.sh target/package/yi-{{version}}-{{target}}.tar.gz

# Bundled skills (14.1) are installed into the global root, not compiled in.
install-skills:
    mkdir -p ~/.yi/skills
    cp -R skills/. ~/.yi/skills/
