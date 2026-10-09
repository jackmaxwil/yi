---
issue: Closes #1136
decision: the tool table stays byte-constant for a session and defers documentation, never a tool: `edit`, `plan`, `todo` and `ask_user` carry a short summary, their parameter schema (`plan` without field prose below the top level) and `read("yi://tools/<name>")`, which serves the whole guide; `read`, `grep`, `bash`, `write`, `get_context` and `ipython` stay whole; an edit refusal names the address; there is no dispatcher tool, and provider-native deferral (`defer_loading`, `tool_search`) is only ever an adapter | the owner: "Non important tools get a pointer to read the full definition"; the tool table fell from 24,754 to 16,423 bytes of a 53,369-byte prefix, and a changed tools array invalidates the whole Anthropic cache, while a guide read lands after the prefix | restore each description from its guide constant and drop the `yi` arm of the resolver
decision: `yi://` is the one address of the documents the prefix names but does not carry: `yi://tools/<name>` and `yi://skills/<name>`, resolved by `read` and `rlm.fetch`; skill pointers from rules, `$name` and the classifier read `Relevant: yi://skills/<name>`, replacing `skill://`, which no scheme ever served | a pointer the model cannot follow is not a pointer; one namespace instead of an alias | point `SKILL_ADDRESS` back at `skill://`
decision: the skills catalog clips every description at 60 chars and drops the file path, its header naming the clip and `yi://skills/<name>` | measured on this machine's 39 `~/.yi/skills`: 7,682 bytes at the old 120-char-plus-path rung, 3,153 clipped at 60, 614 names-only; 29 of the 39 declare no `trigger:`, so a names-only index would leave them a bare name | set `DESCRIPTION_CLIP` back to a ladder and restore the path
raise: crate cli +1, crate runtime +78, crate tools +11, tests +179, comments +5
---
Deferred tool guides (Closes #1136). The long documentation of four tools moves behind an address
the model reads on demand: `edit`'s patch grammar, `plan`'s field prose, `todo`'s long form and
`ask_user`'s description each sit at `yi://tools/<name>`, and the tool keeps a summary that points
there. `read`, `grep`, `bash` and `write` stay whole. The request prefix falls from 53,369 to
45,038 bytes. Skills are addressed the same way: the catalog clips each description at 60 chars
and `read("yi://skills/<name>")` returns the whole skill, which is also what every skill pointer
now names. One-in-one-out: this is not a new top-level feature (it extends fetch §10 and the
tool table §7.1); `ask_user`, two calls in 795 root sessions, is demoted to a one-line stub.
