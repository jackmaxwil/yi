---
id: reuse
question: does it already exist
decider: reader
claim: Deleted / alternatives
severity: {"high": "the change re-implements a function, type, script, config path or mechanism this repository already has", "medium": "a near-copy of code elsewhere that one shared helper should serve, or a second name for a concept the repository already names"}
refute: {"high": 3, "medium": 1}
---
For each new function, type, file, script, flag, config key, error, prompt or doc section the
diff adds, search the repository for one that already does the job (grep its name, its key
words, its shape) before you accept it. Report only a twin you found: cite it as path:line in the
claim and quote the new line in the diff. Copy-pasted blocks count, between new code and old and
within the diff. A finding without the existing twin's address is a guess; leave it out.
