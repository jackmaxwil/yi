# Loud caps

A cap the model cannot see is a lie about the file. Every cap, clamp, page, window, or filter
that shrinks what a tool shows is named in the output it shrinks, at the point of the cut, in
the model's own view, with three facts: what was kept over what existed, the cap that cut it
(name and value), and the one call that gets the rest.

- The notice is a `[…]` row where the cut is. A detail field may carry the same fact for
  `yi stats`; it never carries it instead. A log line is not a notice.
- Every model-facing surface is bound: tool results, `get_context` layers, error text, edit
  responses, background job output, previews, the post-edit ripwire layer.
- A cap that "rarely trips" is still silent when it does. The test for a cap is the test that
  trips it and reads the notice; a cap without that test is unfinished.
- What is not a cap: a window the model asked for (`ranges`, `offset`/`limit`) and a filter
  the model asked for (`def`, `include`, `type`) are honest by construction. A default window
  the model did not ask for is a cap. A fallback (no block resolver, so a fixed window) is a
  cap. A skipped input (a binary file, a file past a walk limit) is a cap.
- Incident 2026-09-04: one landing shipped seven silent caps (skeleton rows, directory heads,
  glob files and per-file heads, grid-check lines, find's fallback window, the near-miss list,
  grep's block fallback), and the audit it forced found four older ones (grep's tag cap, its
  context clamp, its binary skips, `get_context`'s per-file heads). All eleven now speak.
