---
issue: Closes #1720
---
Add pinned static security scans to the shared guardrails. Zizmor audits every Forgejo workflow with hostile fixtures, reasoned suppressions and a shrink-only baseline. Workflow permission and checkout-credential findings are fixed; action tags whose mirror commit cannot be verified offline and two deliberate review behaviors carry inline reasons (Closes #1720).
