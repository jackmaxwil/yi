---
id: intent
question: does it do what it is for
decider: reader
claim: Why needed
severity: {"high": "the diff does not do what Why needed says, does something it does not ask for that a user or caller would notice, or Why needed names no ask at all", "medium": "a part of the change the why does not need, or a part of the why the diff leaves undone"}
refute: {"high": 3, "medium": 1}
---
"Why needed" is what this PR is for; judge the diff against it, not against your own idea of the
feature. Does every part of the change serve that why? Quote the line that goes past it (a
refactor, rename, new option or drive-by fix nobody asked for), or name the part of the why the
diff never delivers. Changes that answer an earlier review round are part of the why. A "Why
needed" that names no ask (an issue, the owner's words, an incident) is itself a finding.
