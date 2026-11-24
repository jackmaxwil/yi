# Desktop

`desktop/` is the Mac app, its own cargo workspace and not one of the seventeen crates: Yi's
`cargo`, `just check` and guardrails never build or read it, and its own gate is
`just check` run inside `desktop/`, on a Mac. It depends on Yi only through `crates/types` and `crates/browser`
by path, so a change to either runs that gate before it lands. A change under
`desktop/` owes Yi's gates nothing but the repository-wide ones that read every tracked file
(public surface, secret scan, blob size, spelling). Its rules and dependency register live in
`desktop/docs/`; the decision is D603 and the design is YI_DESIGN.md §17.5.
