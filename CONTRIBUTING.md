# Contributing

Yi is developed on a private forge. `github.com/jackmaxwil/yi` is a rewritten,
read-only mirror of its `main`. Pull requests from named contributors enter a
manual intake path; arbitrary-contributor admission is not enabled.

Fork `jackmaxwil/yi`, configure the exact email pinned for your login in
`scripts/intake/allowed.json`, and sign every commit with an SSH Ed25519 key
whose public half the owner has added to `scripts/intake/allowed_signers`.
Set `gpg.format ssh`, `commit.gpgsign true` and your signing key, then push a
linear history and open the pull request against `main`. Do not use a GitHub
noreply address or web merge commits. The owner supplies and rotates signing
keys through an ordinary owner-authored forge change.

Intake checks the signature, author and committer on every commit, the pinned
head, size caps, paths, file modes, filenames, content and strict application
to the rewritten mirror base. A refusal arrives as one GitHub comment with the
exact `intake: …` reason and the full checked head. Push a corrected head so
every check runs again. A passing change becomes a draft forge PR labelled
`external` and `autofix:hold`; review does not authorize execution or merge.
No contributor code runs on a runner until the owner applies `run-ci`.

After the owner lands the forge PR, the mirror is rewritten and the GitHub PR
is closed rather than marked merged. Rebase the topic branch onto the new
mirror main with `git fetch https://github.com/jackmaxwil/yi.git main && git
rebase FETCH_HEAD`. Your forge squash commit has a new id and does not claim
to carry the original commit signatures.

A contribution is accepted under the Apache License 2.0 in `LICENSE` (section 5).
