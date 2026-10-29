---
issue: Refs #1719
raise: tests +35, crate cli +11
growth: +11 the only Rust src growth is parsing, validating and applying --no-tools; Python cannot remove the provider tool table without that CLI support
decision: Build stateless, signed, owner-gated GitHub intake into the existing review loop | owner: "Allow Github PRs from named specific contributors with strict rules and high safety --> check for security --> normal Yi reviewbot loop. Primary user will be myself and Max when we are working on different machines" | reversible via removing scripts/intake together with its selfcheck line in scripts/guardrails/check_guardrails.sh, the intake recipe and the journeys line in justfile, the ledger-journeys row, ARCHITECTURE and YI_DESIGN section 22 and the CONTRIBUTING intake text; the --no-tools flag reverts alone
---
GitHub intake stages 0 through 2 add real-producer fixtures, read-only dry-run decisions, strict signed checks, deterministic draft import and an offline real-git journey. The mirror rewrite now shares one helper with intake and retains its ephemeral commit map long enough to translate a mirror base. No schedule or contributor-code execution is enabled.

`yi ask --no-tools` installs an empty tool table, so the intake lens receives only the fenced diff and cannot read the repository or call tools. The draft import always carries `external` and `autofix:hold`, never `run-ci`, `live` or `autofix`. The real transport carries no branch, draft or comment writes yet: they land with the stage-3 hold, and the journey drives the import through an injected transport. Runner hardening, owner CI admission and landing feedback remain stages 3 and 4.
