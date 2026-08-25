---
name: yi-port
description: Port a cited reference span from ref/ into a yi crate following the Appendix A discipline
---

# Porting a reference span

1. Find the mechanism's row in docs/YI_DESIGN.md Appendix A; note the span and the action
   (port verbatim / port adapted / read-only reference).
2. Read only that span. Never open paths named in the section's excise block.
3. Translate: verbatim = 1:1 including constants and exact error strings; adapted = same
   behavior, HAR-shaped (the 020-style rules apply).
4. Carry incident comments on constants and guards; drop every other comment.
5. Run `just check`; if a ratchet moves, justify the growth or shrink the code.
6. If the source diverges from the doc row (line drift, changed behavior), fix the row in the
   same change; note it in the ARCHITECTURE changelog when structural.
7. "Port verbatim" blocked by a banned dependency (textwrap, url, …) is not a
   license to improvise: adapt to the design row's stated contract and fix the
   A-row action to "port adapted" with the reason, in the same change.
8. When the reference ships golden vectors or fixtures (thinking-orbs
   spec/orbs-golden.json, Pi session files), they are the parity contract:
   commit them as test fixtures and compare numbers, not impressions.
