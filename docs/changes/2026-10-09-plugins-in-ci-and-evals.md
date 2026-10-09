---
issue: Closes #1041
raise: tests +58
---
CI and the evals get every plugin `yi plugin add --all` installs. The postmerge lane and the tbv4 weekly job install them before they run, cached on the forge by the plugin table's hash, and a failed install fails the job instead of running without them. A new `plugins-live` job runs on a pull request that touches the plugin table or ripwire's readers. It and the postmerge journeys run `the_pinned_ripwire_answers_in_the_shapes_yi_reads`, which drives an edit and a `get_context` call through the real pinned ripwire on a two-file Python repository and checks that the broken caller is named. The harbor adapter uploads each installed plugin to `/usr/local/bin` in a task container in path mode, checks it answers `--version` there, and adds `+plugins{ripwire@0.6.5}` to the fingerprint's mode. `EVAL_PLUGINS=none` (or a list of names) picks the arm, and URL mode carries none and says `+plugins{}`. PR test jobs stay plugin-free.
