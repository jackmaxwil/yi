---
version: 0.922.0
decisions: D548
issue: Closes #2023
raise: tests +98, comments +3, crate cli +145
decision: A release goes out every night from CI, on the forge and on GitHub, and yi update never installs an older release unless asked | `yi update` read a latest release that had stood at 0.799.0 for a week while main reached 0.919.0, and installed it over 0.898.0, whose config the old binary could not read | delete release.yml and restore the old `already` check; releases go back to `just publish` and `scripts/mirror/publish.sh` by hand. Amends D274: a Mac binary linked off a Mac by zig keeps its function-starts table, since only Apple's ld takes -no_function_starts
ledger~: `yi update` | from the latest signed release, and refuses | from the latest signed release, refuses a release older than itself unless run with `--downgrade`, and refuses
ledger~: `yi update` | the first install is | a public build reads GitHub's latest release instead of the forge's; `.forgejo/workflows/release.yml` publishes both every night, a Mac archive linked through zig against a trimmed macOS SDK and a static x86_64 Linux one; the first install is
---
`yi update` no longer installs a release older than the yi it replaces. It says `already 0.898.0, newer than the latest release 0.799.0; yi update --downgrade installs it` and leaves the binary alone; `--downgrade` installs the older one. It also runs the downloaded binary's `--version` before replacing anything, so a release that cannot start on this machine, which nothing checks for a Mac binary cross-linked on Linux, is refused and the old yi stays.

A release now goes out every night. `.forgejo/workflows/release.yml` builds main's version for Apple-silicon Macs and for static x86_64 Linux on the forge's runner, signs both with the release key, and publishes them on the forge. It then builds the same version from the public mirror's tree and publishes it on GitHub, where a public build's `yi update` now looks. Each release stays a draft until every archive is uploaded, so a night that dies part way leaves nothing `yi update` reads, and the next night deletes the draft and starts again. The GitHub half builds only the mirror commit the forge release became, checked against filter-repo's commit map. Forge releases past the newest seven are deleted.

The release key is new. The old key's passphrase was lost, so it could sign nothing, here or in CI; `allowed_signers`, `scripts/install.sh` and the README now trust `SHA256:Tb4DlV9Kzm6hIEEnTNTjGNPv4msLyf4zlFPIZZlpsOE`. A yi built before this change trusts only the old key, so its `yi update` refuses the first release signed with the new one; reinstall it once with `scripts/install.sh`.
