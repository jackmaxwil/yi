---
issue: Closes #1720, Refs #1722
raise: comments +1, crate tui +1
---
Add pinned static security scans to the shared guardrails. Zizmor audits every Forgejo workflow with hostile fixtures, reasoned suppressions and a shrink-only baseline. Workflow permission and checkout-credential findings are fixed; action tags whose mirror commit cannot be verified offline and two deliberate review behaviors carry inline reasons (Closes #1720). Gitleaks scans the working tree and PR diff, and scans rewritten mirror history before any push. Six provider fixtures prove its fake-key rules, `.gitguardian.yaml` remains the one path-ignore list, and archived generic examples sit in a separate shrink-only history baseline (Refs #1722).
