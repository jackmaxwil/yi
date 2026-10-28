## Delegation

RLM subagents from the kernel: a bare call is a reader that answers one question from the
`partition` it is handed (`rlm.ask(q, [url])`); `role="worker"` makes one change under the
project's rules; `role="root"` is a full child that executes a todo with a check; `rlm.status()`
shows them.

    SCHEMA = {"type": "object", "required": ["evidence"], "properties": {"evidence": {"type": "string"}}}
    r = await rlm.ask("Which line of notes.txt names the sea? Quote it.", ["local://notes.txt"], schema=SCHEMA)

Two kinds of child, named apart because their rules differ. A reader
explores, reconnoitres, reviews, compares, or reads a reference; it is
the common case, cheap, and walled. A writer executes one todo with a
check; it is the rare case and the one that needs ownership.

1. Solo, readers, or writers. Solo when the work is one file, one
   checker, or a chain where each step needs the last. Readers when the
   questions outnumber the turns you can spend reading: map a repository
   by area, test three or more candidate causes at once, read a corpus or
   a vendor tree, compare several implementations, review your own
   finished work cold. Writers when three or more units of change are
   independent (no shared file, no edge between them), each a session's
   worth, each with a check written before the child starts. Never
   delegate the reasoning the answer turns on: a reader brings evidence,
   you conclude.
2. A reader is walled and cheap: a bare `rlm.run`, or `rlm.ask(question,
   partition)`, gets a short reader prompt, `read` and `grep`, three turns
   and no writes; it runs in your tree with no worktree, on a model whose
   `rlm.find_models` cost is one input, with one question, the lines it needs
   inlined as `partition` URLs, and findings shaped
   `{path, line, claim, evidence}` where `evidence` is the quoted line. A
   reader's claim is data: open the cited line before you build on it; a
   claim with no citation is dropped at the schema seam, not argued with.
3. The flow. The plan is the hand-off. Lift a todo into it only when a
   child executes it, with its contract; the engine starts it, submits its
   finish and accepts or refuses it. Accepted is verified: its notice
   quotes the checks that passed; never rerun them.
4. Ownership and waiting. Readers own nothing and share your tree. Two
   writers never own one file: `isolation='worktree'` each and
   `merge_worktree` in dependency order, or a `deny_write` list that is
   the complement of the scope. Keep working what you kept;
   `await rlm.wait(120)` only when the next step needs a result, and read
   the `counts` and the `states` of the members that moved. Between waits
   `rlm.status()` is the full roster: `needs_you` gets
   `send(name, text, reply_to=id)`; `stuck` gets its tail
   Collect with `await h.result(schema=SCHEMA, timeout=420)`; reap with
   `rlm.delete_subagent`. Depth is one unless the config raises it.
5. Data stays in kernels. A large result comes home by `rlm.put(name, obj)`
   and your `rlm.get(name)`, a file in a worktree by
   `tree://<name>/<path>`, a live value by
   `await rlm.fetch("kernel://<name>/<var>")`. The transcript carries the
   digest and the decision, never the data.

    SCHEMA = {"type": "object", "required": ["outcome"], "properties": {"outcome": {"type": "string"}}}
    h = await rlm.run(brief, name="foo", role="root", isolation="worktree")
    r = await h.result(schema=SCHEMA, timeout=420)

The plan is for delegation, and while open it is the todo list. Lift
todos into the plan tool when work will be handed to children, when tasks
carry checks the runtime should run, or when the dependency order matters
more than the reading order.
