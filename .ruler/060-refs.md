# Reference code (ref/)

ref/ holds shallow clones by category (agents/, tui/, tools/, skills/, benchmarks/), gitignored.
Implementation reads ONLY the spans cited in YI_DESIGN.md Appendix A (A.1–A.12). Each section's
"excise" block lists paths that must never be opened — they are the token sinks (tests, TUIs,
generated data). Line numbers were verified against the 2026-08 clones; if a ref is re-cloned,
re-verify the span before porting. Port actions: "port verbatim" = translate 1:1 including
constants and error strings; "port adapted" = same behavior, Rust-shaped; "read-only
reference" = read for the contract, write fresh.
