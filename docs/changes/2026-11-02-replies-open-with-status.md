---
issue: Refs #1862
decision: a reply opens with its status: the `answer` tool's `summary` heads the reply the user reads, written as `Problem:`, `Ready:` or `Done:` and one plain sentence, a summary alone is a whole reply, a plain-text reply already on screen stands as written and a `Problem:` or `Ready:` summary that could not head it is refused, so it comes back over new text; the todo gate sends a `Done:` answer back while an item is blocked until its send-back ladder tops out, and a plain-text `Done` once; text that parses whole as JSON stays bare, and no line is added that would change the JSON a structured reader parses out of the reply; the identity prompt replaces D523's Voice with a stance (do the work, stop only at what cannot be undone or speaks for the user, hand over one action that cannot go wrong), facts kept apart from marked opinions, and register matching (the user's language, length and tone, swearing only after the user does, plain words with no code, file names, commands or jargon unless the user writes them, the long version when asked for depth), and a reply to another agent counts as saved work; D523's two-short-paragraphs rule goes (amends D523); doctrine lets a diagnosis end in its fix and keeps an error's exact text for saved work or a user who asks, and the repository's done bar, guardrails rule and refute skill say the same, while commits still wait for the user's word | the owner: "I dont want to ever be handed things to do", "I want Yi to have opinions about things but clearly delineate their opinion from absolute ground truth", "In general, try to match the user" and "I don't want to look at files, code, syntax, jargon"; every answer already carried a required `summary` that no screen showed; a `--here` session has no lane, so committing unasked stays off | restore identity.md, the doctrine and orchestrate sentences, the three ruler lines and prose.py's rule and stems, return `headed` to the bare text, refuse a summary-only answer again and drop the `Done:` check; no schema, wire shape or config key moves
ledger~: identity + doctrine: the comprehensive prompt | the Voice section states the reply style as nine rules (short sentences, list-first, two short paragraphs of prose, answer first, detail on request, no restating, no closing summary, no rule talk, brevity keeps errors, warnings and undone items) and puts every banned tell in one section | the reply opens with the `answer` summary as `Problem:`, `Ready:` or `Done:` and one plain sentence, a `Done:` answer over a blocked item is sent back until the send-back ladder tops out and a plain-text one once, and the prompt teaches a stance, facts apart from opinions and register matching in plain words; eight measured rules stay (short sentences, list-first, answer first, detail on request, no restating, no closing summary, no rule talk, brevity keeps errors, warnings and undone items), and one section lists every banned tell
raise: tests +280, comments +7, crate runtime +89, crate tools +1
---
Yi's replies open with a status line. The `answer` tool's `summary`, which every answer
already carried and no screen showed, now heads the reply as `Problem:`, `Ready:` or `Done:`
and one plain sentence, a summary alone is a whole reply, and a `Done:` is sent back while a
todo is blocked. A `Problem:` or `Ready:` line that a reply already on screen would hide is
refused, so it is sent again over new text. JSON answers reach the programs that read them
unchanged. The identity prompt is rewritten around how the owner wants to be answered: Yi does
the work itself and stops only at what cannot be undone or speaks for the user, keeps facts
apart from opinions, and matches the user's language, length and tone, in plain words with no
code, file names or jargon unless the user writes them. Doctrine now lets a diagnosis end in
its fix.

The guardrails lane now names a gate that failed without printing a `FAIL` row. This branch's own
`gate (guardrails)` job printed `guardrails: 1 failing` with nothing above it, and the failure took a second
pass to attribute because the log never said which check it was. `drain` and the reporter the two dist gates
print through both record the gate's command and exit code, and report `FAIL <cmd> (exit N, printed no FAIL
line)` only when the gate printed nothing at all: a gate that printed its own traceback keeps it, because that
tail is what `scripts/pr_autofix.py` hands a repair turn. The autofixer's hand-rolled environment save and
restores are one `without_env` helper.
