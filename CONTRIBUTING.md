# Contributing

Yi is developed on a private forge. `github.com/jackmaxwil/yi` is a rewritten,
read-only mirror of its `main`. Pull requests from named contributors enter a
manual intake path; arbitrary-contributor admission is not enabled.

## Before the first pull request

Ask the owner to pin your GitHub login, your commit email and the SHA256
fingerprint of an SSH Ed25519 key in `scripts/intake/allowed.json` and
`scripts/intake/allowed_signers`. Those two files stay on the forge, so you
cannot read them from the mirror; the owner confirms what is pinned. The email
must match letter for letter, case included: `Jane.Doe@example.com` is refused
when `jane.doe@example.com` is pinned. Do not use a GitHub noreply address.

GitHub issues are off on the mirror. The owner opens the forge issue for your
work and gives you its number; cite it as `Closes #N` in your change file
(`docs/changes/`) and in the pull request description.

## From a GitHub fork

The `just commit`, `just push`, `just pr` and `just land` recipes drive the
private forge and do not work from a mirror clone. Use git and GitHub directly.
Fork `jackmaxwil/yi`, then set up the clone once:

```bash
git clone https://github.com/<you>/yi.git && cd yi
git remote add upstream https://github.com/jackmaxwil/yi.git
git config user.email <the pinned email, exactly>
git config gpg.format ssh
git config user.signingkey ~/.ssh/id_ed25519.pub
git config commit.gpgsign true
echo "<the pinned email> $(cat ~/.ssh/id_ed25519.pub)" > ~/.ssh/allowed_signers
git config gpg.ssh.allowedSignersFile ~/.ssh/allowed_signers
```

Add the same public key to your GitHub account as a signing key. Start each
topic from the mirror's current `main`, commit, check, push to your fork and
open a draft:

```bash
git fetch upstream main && git switch -c <topic> upstream/main
git log --format='%h %G? %ae %ce' upstream/main..
git push -u origin <topic>
gh pr create --draft --repo jackmaxwil/yi --base main --head <you>:<topic>
```

Every line of that `git log` must show `G` and the pinned email twice. Never
push a branch to `jackmaxwil/yi` itself: its `main` is rewritten from the forge
and nothing else there is read.

## What the importer refuses

Intake checks the signature, author and committer on every commit, the pinned
head, size caps (20 commits, 40 files, 4,000 lines by default), paths, file
modes, filenames, content and strict application to the rewritten mirror base.
History must be linear: rebase, never merge.

These paths are refused outright, because changing them changes what the gates
or the build trust: `scripts/guardrails/` (the checks and their baselines),
`scripts/hooks/`, `scripts/intake/`, `scripts/mirror/`, `justfile`, `.github/`,
`.forgejo/`, `vendor/`, `deny.toml`, `rust-toolchain.toml`, `allowed_signers`,
any `build.rs` or `.cargo/`, and every path the mirror omits. Dependency
changes are refused too. The full rule is `deny_rule` in
`scripts/intake/bridge.py`. When your change pushes a size baseline or needs a
check changed, leave the file alone and say so in the pull request
description; the owner makes that edit on the forge.

Run `just lint` and `just test` before pushing. Some `just check` guardrails read files or history that only
the forge has and fail on a mirror clone; name any such failure in the pull
request description rather than editing the check.

## After intake

A refusal arrives as one GitHub comment with the exact `intake: …` reason and
the full checked head. Push a corrected head so every check runs again. A
passing change becomes a draft forge PR labelled `external` and `autofix:hold`;
review does not authorize execution or merge. No contributor code runs on a
runner until the owner applies `run-ci`.

After the owner lands the forge PR, the mirror is rewritten and the GitHub PR
is closed rather than marked merged. Rebase the topic branch onto the new
mirror main with `git fetch upstream main && git rebase upstream/main`. Your
forge squash commit has a new id and does not claim to carry the original
commit signatures.

A contribution is accepted under the Apache License 2.0 in `LICENSE` (section 5).
