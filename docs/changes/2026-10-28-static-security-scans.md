---
issue: Closes #1720, Refs #1722
raise: crate tui +1, comments +1
---
Add pinned static security scans to the shared guardrails. Zizmor audits every Forgejo workflow with hostile fixtures, reasoned suppressions and a shrink-only baseline. Workflow permission and checkout-credential findings are fixed; action tags whose mirror commit cannot be verified offline and two deliberate review behaviors carry inline reasons (Closes #1720). Gitleaks scans the working tree and PR diff, and scans rewritten mirror history before any push. Seven provider fixtures prove its fake-key rules; `.gitguardian.yaml` remains the one path-ignore list and gitleaks derives only its planted-fake entries, so a real source path on that list is still scanned, and archived generic examples sit in a separate shrink-only history baseline (Refs #1722). Vendored Semgrep rules cover the trial's Python findings plus Yi subprocess and production-Rust laws. MD5/SHA1 uses and a shell pipeline are fixed; direct argv calls and two temporary-directory uses carry reasons. Every scanner family has hostile and clean fixtures and a disabled-rule selfcheck.

The gate provisions its own scanners: `check_scan.py` runs the pinned gitleaks installer before it
scans in CI and searches the directory that installer writes, so no workflow step or autofix hook
carries that mapping a fourth time and no version string stands in for the gate's own install. The
scan baseline keeps a row for every scanner that ran with no findings, which is what stops a family
whose rows all fell to zero from reading as brand new and seeding its growth unchecked.
