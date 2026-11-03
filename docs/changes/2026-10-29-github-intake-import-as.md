---
version: 0.894.0
issue: Refs #1794
---
An imported contributor's commits can name the identity that contributor chose. An
optional `import_as` object in their `scripts/intake/allowed.json` entry, `name`,
`email` and `consent`, rewrites the imported squash commit's author and its
`Signed-off-by` trailer; `consent` records when and how they agreed, and an entry
without it, or with an email that is not a plain address, refuses the import by
naming the contributor and the field. The rewrite happens where the imported commit
is produced and nowhere earlier, so the pinned email, the signature, the caps and
every other check still judge the commits as submitted; a contributor with no
`import_as` imports exactly as today, and the author is never the owner.

Authorship stays honest and auditable. The rewrite moves no author date, message body
or tree, the committer is still `yi-bot`, and the source account is kept in the import's
audit comment rather than in the public commit objects: that record now names the GitHub
login, the pull request URL, the original head sha and the verified signer's fingerprint,
which is read from `verify-commit --raw` and only when the commit verified against the
pinned signers. `just intake --dry-run` prints the identity it would apply.

`docs/FORGE.md` states what the mapping does and does not do for the owner and the
contributor. The fixture is a second contributor entry beside the recorded ones;
`allowed.json` gains no contributor entry, because the owner adds those.
