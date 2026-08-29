# vendor/seatbelt

`seatbelt_base_policy.sbpl` is a verbatim copy from openai/codex
(`codex-rs/sandboxing/src/seatbelt_base_policy.sbpl`, Apache-2.0, see
LICENSE-Apache-2.0.txt), which derives it in turn from Chrome's macOS sandbox
policy. It is the deny-by-default base: process, sysctl, IOKit, mach-lookup and
pty rules that let an ordinary build or test run at all.

Everything Yi layers on top of it (readable roots minus the credential stores,
writable roots, the absence of any network rule) is generated in
`crates/tools/src/sandbox.rs`, ported from the same crate's `seatbelt.rs`. The
network policy file is deliberately not vendored: Yi's sandbox runs with egress
off, and the base policy denies by default, so there is nothing to include.
