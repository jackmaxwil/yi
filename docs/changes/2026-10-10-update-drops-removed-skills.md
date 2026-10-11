---
issue: Closes #1161
raise: crate cli +22, comments +1
---
`yi update` and `scripts/install.sh` now replace each top-level skill directory the release ships (`~/.yi/skills/yi/` today) instead of writing over it, so a skill the release no longer carries is removed. `skills/yi/grid`, left behind when grid was deleted, disappears from the catalog on the next update. Skills a user keeps in their own directories under `~/.yi/skills` are untouched, and a failed update still leaves the old binary and skills in place. `just install-skills` already behaved this way.
