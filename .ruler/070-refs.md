# Reference code (ref/)

ref/ holds shallow clones by category (agents/, tui/, tools/, skills/, benchmarks/), gitignored.
Implementation reads ONLY the spans cited in YI_DESIGN.md Appendix A (A.1–A.12). Each section's
"excise" block lists paths that must never be opened — they are the token sinks (tests, TUIs,
generated data). Line numbers were verified against the 2026-08 clones; if a ref is re-cloned,
re-verify the span before porting. Port actions: "port verbatim" = translate 1:1 including
constants and error strings; "port adapted" = same behavior, Rust-shaped; "read-only
reference" = read for the contract, write fresh.

Port actions describe the initial translation only — a one-time copy of features Yi wanted,
done verbatim to de-risk bring-up. Once landed, the code is Yi's: no upstream tracking, no
long-term parity obligation, edit freely (the design doc, not the ref, is the authority).
The one deliberate byte-level compatibility contract is the Pi v4 session-file format — an
interop anchor, not code parity.
