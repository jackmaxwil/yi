default: check

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

build-dist:
    cargo build --profile dist -p yi-cli

guardrails:
    bash scripts/guardrails/check_guardrails.sh

check: fmt-check clippy guardrails
