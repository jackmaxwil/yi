# Context gathering

A source the model cannot find does not exist, and a source it finds by accident is a leak.
Every feature that brings context to the model — a scheme, a search verb, a memory, an
integration — obeys these eight rules.

1. **Inject pointers in-band.** A source's address rides the payload the model must consume:
   the spawn brief, a tool result row, an error string. Doctrine and the skill catalog are
   fallbacks, never the only route; the catalog clips a description at 60 chars. A pointer is
   priced by how often it is paid: per spawn before per request.
2. **One namespace, resolved by name.** A source is a URI scheme the one `Resolver` serves
   through `read`; a search returns addresses `read` opens. The model gets aliases (`self`,
   `parent`, a child's name) the resolver maps to ids; it is never asked to know an id. A new
   source adds a scheme, not a tool.
3. **Role-invariant resolution.** Every role (root, reader, worker, walled) reaches the shared
   addresses through the same backends, or gets a typed refusal naming why. A role-matrix test
   holds it, because the failure is silent: green suites shipped a reader resolver that could
   open no session but its own.
4. **Spend context by recoverability.** Data with no other source of truth — a person's words,
   a decision, expired output — gets the widest per-item cap. Data its authority still serves
   — a file, a rerunnable command — gets the narrowest and is read from the authority, never
   from a transcript's copy, which is stale by the next edit.
5. **Bounded, self-describing views.** Every model-facing view has a default bound: unpaged
   reads, listings, one-line summaries. Every cut follows 045: kept of total, the cap's name
   and value, the continuation call. A per-line cut in a compact view is a cut.
6. **Canonicalize before authorizing.** An alias or rewrite resolves to its target, and the
   access policy is judged against every spelling of it. A policy that names the target in a
   form the canonical id does not match fails closed. Never point a model at an address its
   own wall refuses.
7. **External data is fenced and attributed.** A result from the network, the forge or an MCP
   server carries its origin, renders inside a data fence, and is never written into the
   session ledger as a first-party entry nor re-indexed as the agent's own history.
8. **Prove it on the target model, graded on the actor.** A context feature lands with a paired
   run against the fork binary on the model it serves (GLM 5.3 flash by default), k repeats an
   arm, graded on the acting agent's own final output, on tasks the baseline can fail, plus a
   `yi ask` dogfood log. Never gate on a metric the feature moves by construction.

- Incident 2026-10-10 (#1184): Yi had session search, recall and MCP search, and in 300 sessions
  the model called none of them. A child sent to find what its parent was told walked the tree:
  0/3 on the fork, 1/3 with a doctrine line, 3/3 with the address in its brief. A dogfood read
  put 4.86 MB of one session in a tool result, a 120-char cut hid the user's sentence the child
  needed, and a reader's resolver could not open its parent. Every one passed its unit tests.
