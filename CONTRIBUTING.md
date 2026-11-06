# Contributing

Yi is developed on a private forge. `github.com/jackmaxwil/yi` is a rewritten,
read-only mirror of its `main`. Pull requests from named contributors enter a
manual intake path; arbitrary-contributor admission is not enabled.

## Before the first pull request

Send the owner your GitHub login, your commit email and the public half of an
SSH Ed25519 key (the whole `id_ed25519.pub` line, not only its fingerprint).
The owner pins them in `scripts/intake/allowed.json` and
`scripts/intake/allowed_signers`. Those two files stay on the forge, so you
cannot read them from the mirror; the owner confirms what is pinned. The email
must match letter for letter, case included: `Jane.Doe@example.com` is refused
when `jane.doe@example.com` is pinned. Do not use a GitHub noreply address.

GitHub issues are off on the mirror. The owner opens the forge issue for your
work and gives you its number; cite it as `issue: Closes #N` in your change
file under `docs/changes/` and in the pull request description.

You need rustup, `just`, and Python 3.11 or newer on `PATH`: every `just`
recipe looks up the interpreter first, and the Python 3.9 macOS ships with
stops them all.

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
f="$(git rev-parse --absolute-git-dir)/allowed_signers"
echo "<the pinned email> $(cat ~/.ssh/id_ed25519.pub)" > "$f"
git config gpg.ssh.allowedSignersFile "$f"
```

Add the same public key to your GitHub account as a signing key. Commit only
from this clone: a commit GitHub makes for you (the web editor, "Commit
suggestion", "Update branch") is signed by GitHub, not by your key, and is
refused. Start each topic from the mirror's current `main`, commit, check,
push to your fork and open a draft:

```bash
git fetch upstream main && git switch -c <topic> upstream/main
git log --format='%h %G? %ae %ce' upstream/main..
git diff --check upstream/main...
git push -u origin <topic>
gh pr create --draft --repo jackmaxwil/yi --base main --head <you>:<topic>
```

Every line of that `git log` must show `G` and the pinned email twice, and
`git diff --check` must print nothing. Never push a branch to `jackmaxwil/yi`
itself: its `main` is rewritten from the forge and nothing else there is read.

## What the importer refuses

Intake checks every commit's signature, author and committer, then the pinned
head and the whole diff against the rewritten mirror base. History must be
linear: rebase, never merge. The diff must apply with no whitespace errors.

Size caps are set per login; today every login has 20 commits, 40 files,
4,000 lines (added plus deleted) and a 150,000-byte patch. Renames are not
detected, so a moved file costs two files and every one of its lines.

These paths are refused outright, because changing them changes what the gates
or the build trust: `scripts/guardrails/` (the checks and their baselines),
`scripts/hooks/`, `scripts/intake/`, `scripts/mirror/`, `skills/yi/pr-review/`,
`justfile`, `.github/`, `.forgejo/`, `vendor/`, `deny.toml`,
`rust-toolchain.toml`, `allowed_signers`, `.gitguardian.yaml`, any `build.rs`
or `.cargo/`, and every path the mirror omits. Any edit to a `Cargo.toml` or
`Cargo.lock` is refused too, and so is an added `[dependencies]`-style section
header or `proc-macro = true` line in any file. No new crate comes in this way.
The full rule is `deny_rule` in `scripts/intake/bridge.py`.

Added lines are also refused when they carry a URL, an IP address,
`localhost`, a hostname, an email address, or `unsafe`, and so are an
executable bit, a binary file and a non-ASCII filename. Some of these are
refused pending owner review rather than for good: name any you need in the
pull request description. When your change pushes a size baseline or needs a
check changed, leave the file alone and say so in the description; the owner
makes that edit on the forge.

Run `just lint` and `just test` before pushing. Some `just check` guardrails
read files or history that only the forge has and fail on a mirror clone; name
any such failure in the pull request description rather than editing the check.

## After intake

The owner runs intake: tell the owner when the pull request is up and each time
you push a corrected head. A refusal arrives as a GitHub comment with the exact
`intake: …` reason for the first check that failed and the full checked head;
fix it and push again, and every check runs again. A passing change becomes a
draft forge PR labelled `external` and `autofix:hold`; review does not
authorize execution or merge. No contributor code runs on a runner until the
owner applies `run-ci`.

After the owner lands the forge PR, the mirror is rewritten and the GitHub PR
is closed rather than marked merged. Your forge squash commit has a new id and
does not claim to carry the original commit signatures, so start the next topic
fresh from `upstream/main` rather than rebasing the old one onto it.

A contribution is accepted under the Apache License 2.0 in `LICENSE` (section 5).
