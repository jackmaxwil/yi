# Ripwire's skills on demand, and a skill for every major tool

Status: proposal rev 2 (self-review at the end; where they disagree, it wins), 2026-10-10. Branch `jack/ripwire-skills-plan` off main `ffbc2e5b`. It builds on
#1142 (`yi plugin`, the plugin table) and #1143 (plugins in CI and evals); both were open and
unmerged when this was written. ✓ marks what exists on main or in #1142, ✚ what is new.

## The owner's words

> 1) make this a persisted durable enforced principle: every major agent tool/feature needs a
> corresponding skill
> 2) fresh worktree off main: plan inclusion of all ripwire skills on demand. lazy loaded. pointer
> to full skill

The answers to the grilling round, verbatim:

| Question | Answer |
|---|---|
| What counts as major | "Model tools + plugins (Recommended)" |
| How it is enforced | "Gate + shrinking gap list (Recommended)" |
| Which ripwire skills | "All but those two (Recommended)" |
| How they appear in the catalog | "One pointer line (Recommended)" |

On "all but those two": the question named `ripwire-mcp` and `hermes`. The 0.6.5 release tarball
does not ship `hermes`; it exists only in ripwire's repository. So the set is the release's 17
skills minus `ripwire-mcp`, which is **16**.

## Summary

Lazy loading already exists in Yi. The skill catalog carries one line per skill, and
`read("yi://skills/<name>")` returns the whole skill only when the model asks (YI_DESIGN §7.8,
`crates/runtime/src/skills.rs`, `crates/runtime/src/fetch/docs.rs`). The plan therefore adds no
loader. It adds:

1. ripwire's skills, unpacked from the same verified archive as the binary;
2. a way for a skill to be readable without being listed;
3. one Yi-written `ripwire` index skill that is the single catalog line pointing at the rest.

A gate makes the principle enforced: every model tool and every plugin is covered by a skill, or
sits on a pending list that may only shrink.

| Primitive | What it is | Status |
|---|---|---|
| Catalog line + `read("yi://skills/<name>")` | the lazy load: a ~60-character pointer in the prompt, the body on demand | ✓ `skills.rs`, `fetch/docs.rs` |
| Plugin row | pinned archive, sha256 per target, published by one rename | ✓ #1142 `crates/tools/src/plugin.rs`, `crates/kernel/src/pinned.rs` |
| Plugin skills | the row names skill directories to unpack beside the binary, from the same verified archive | ✚ `Plugin::skills`, `pinned::install` publishes a member set |
| Unlisted root | an installed plugin's `skills/` directory is discovered for `read`, never listed in the catalog | ✚ `Skill::listed` in `skills.rs` |
| `requires:` | frontmatter that lists a skill only while the named plugin is found | ✚ `skills.rs` |
| Skill-relative files | `yi://skills/<name>/<file>` reads a file beside `SKILL.md` | ✚ `fetch/docs.rs` |
| `covers:` | frontmatter that names what a skill covers: `tool:bash`, `plugin:ripwire` | ✚ read only by the gate |
| Coverage gate | every `tool:*` and every plugin covered, or pending; pending only shrinks | ✚ `scripts/guardrails/check_skill_coverage.py` + `baselines/skill_pending.json` |

## Evidence

- `skills/yi/grid/SKILL.md` still ships on main, although grid was deleted in #1025. Its
  replacement, ripwire, has no skill at all, only the 18-line card
  (`crates/runtime/src/prompts/ripwire.md`). Nothing failed either way, which is what the gate is
  for.
- The model-facing tools, from `scripts/guardrails/baselines/tool_surface.json` (`tool:*` keys):
  `annotate`, `bash`, `edit`, `get_context`, `grep`, `ipython`, `plan`, `read`, `todo`, `write`.
  Only `plan` has an obvious skill today (`skills/yi/plan`).
- The ripwire 0.6.5 release tarball ships 17 skills, 292,937 bytes of text in all; `ripwire-mcp`
  alone is 34,844 bytes. Three skills point to files beside their `SKILL.md`: `ripwire-orient`
  (`map-before-you-read.md`, `compress-ladder.md`), `ripwire-fresh-eyes` (`lint-rules.md`) and
  `ripwire-quality-bar` (`quality-metrics.md`). `yi://skills/<name>` serves only `SKILL.md` today,
  so those links would dangle.
- Descriptions use YAML folded scalars (`description: >`), which `skills::frontmatter` already
  reads.

## Design

### 1. Install: the skills come with the binary

`Plugin` gains `skills: &'static [&'static str]`, the skill directories to publish. For ripwire
that is the 16 names. `pinned::install` takes a member set rather than one member: the binary
plus every regular file under `<prefix>/skills/<name>/` for each listed name. All of them come
from the one archive whose digest already matched, and are published by the same staging rename.

```
~/.yi/plugins/ripwire/0.6.5/ripwire
~/.yi/plugins/ripwire/0.6.5/skills/ripwire-orient/SKILL.md
~/.yi/plugins/ripwire/0.6.5/skills/ripwire-orient/map-before-you-read.md
…
```

An install from #1142 holds the binary alone. `Plugin::installed` therefore also requires every
listed skill's `SKILL.md`, so `yi plugin sync` repairs such an install. If #1142 has not merged by
the time this is built, stage 1 lands on its branch instead.

### 2. Discovery: readable, not listed

`skills::discover` appends, after the user, repository and home roots, the `skills/` directory of
each plugin installed at its pinned version, with `listed: false`. The catalog skips unlisted
skills; `read("yi://skills/ripwire-orient")` finds them. The first root still wins a name, so a
user's own `ripwire-orient` shadows the plugin's.

A ripwire found only on PATH (Homebrew) brings no skills under Yi's control. The index says so:
a read that answers `skill not found` means ripwire came from PATH, and `yi plugin add ripwire`
installs the skills.

### 3. The pointer: one Yi-written index skill

`skills/yi/ripwire/SKILL.md` replaces `skills/yi/grid/SKILL.md`, which is deleted. It ships the way
every `skills/yi` skill does (`just install-skills`, `yi update`).

```yaml
---
name: ripwire
description: Code map: pick the ripwire skill for a task; 16 on demand
requires: ripwire
covers: plugin:ripwire
---
```

The body is a table of the 16 skills, each row giving when to use it in one line and its address
`yi://skills/ripwire-<name>`. With `requires: ripwire` the catalog lists it only while ripwire is
found, so a user without ripwire pays nothing. The prompt cost is one catalog line, about 100
bytes, against about 1.3 KB for 15 or 16 lines.

### 4. Files beside a skill

`yi://skills/<name>/<path>` reads `<skill dir>/<path>`. The path is normalized; a `..` component,
an absolute path, or a path that resolves outside the skill directory is refused. It is read under
`deny_read`, as `SKILL.md` is. `skills/yi/pr-review/probes/*.md` gets the same addresses for free.

### 5. Trust

Plugin skills are third-party text the model follows, pinned by the same sha256 as the binary that
already runs on every edit. They are home-installed by the user's own `yi plugin add`, so they
take the trust of `~/.yi/skills`, not of a repository skill. They are reached only by an explicit
read; nothing auto-attaches them.

## The principle, enforced

| Piece | Shape |
|---|---|
| Major | every `tool:*` key in `tool_surface.json` (the model's tool list, already kept by its own gate) and every `name:` in the `PLUGINS` table |
| Coverage | a skill says what it covers: `covers: tool:plan` or `covers: plugin:ripwire` (a comma list) |
| Gate | `check_skill_coverage.py` reads `skills/**/SKILL.md` and `python/skills/*/SKILL.md`. It fails on: a major item neither covered nor pending; a pending item that is now covered (remove it: the list only shrinks); a `covers:` naming a tool or plugin that no longer exists (the grid case). `--selfcheck` disables each check in turn and must fail for that check's reason. |
| Pending | `baselines/skill_pending.json`, seeded with today's gaps: `tool:annotate`, `tool:bash`, `tool:edit`, `tool:get_context`, `tool:grep`, `tool:ipython`, `tool:read`, `tool:todo`, `tool:write` |
| Rule text | `.ruler/0NN-skill-per-tool.md` with the owner's sentence, propagated by `npx @intellectronica/ruler apply` |
| Decision | one D-row through the change file: a major tool or plugin ships with a skill, and deleting it deletes the skill |

`skills/yi/plan` gains `covers: tool:plan`. The pending list then holds nine tools; each one leaves
the list the day its skill lands.

## Composition

| Capability | Composed from |
|---|---|
| ripwire skills on demand | plugin row ✓ + member set ✚ + unlisted root ✚ + `read` ✓ |
| One pointer line | catalog ✓ + `requires:` ✚ + index skill ✚ |
| Skill sub-files | `yi://` resolver ✓ + skill-relative path ✚ |
| Principle enforced | tool surface baseline ✓ + plugin table ✓ + `covers:` ✚ + gate ✚ + shrink-only pending list (the repo's existing idiom) ✓ |
| Stale skill caught | gate's unknown-target check ✚ |

## Build order

| Stage | Demo | Gate |
|---|---|---|
| 1. Member-set install, unlisted plugin root, `requires:`, skill-relative files, index skill replacing grid's | fresh HOME: `yi plugin add ripwire`; the catalog shows one `ripwire` line; `read("yi://skills/ripwire")`, then `yi://skills/ripwire-orient`, then its `map-before-you-read.md`; without ripwire, no line | `just check`; tests at the limits: a skill set missing one `SKILL.md` reinstalls; the 16 published atomically with the binary; `..` refused; an unlisted skill never in the catalog; the stage 2 live journey (#1143) extended to read one plugin skill from the real release |
| 2. Coverage gate, pending seed, `covers:` on `plan` and `ripwire`, `.ruler` rule, D-row | deleting the index skill fails the gate on `plugin:ripwire`; a skill covering `tool:grid` fails as unknown | `--selfcheck`; `just check` |
| 3. One skill per pending tool, each its own issue on the forge | each lands with its tool's skill and removes one pending row | the gate |

Stages 1 and 2 are one issue each. Stage 3 is nine issues, registered when stage 2 lands.

## Size

- Stage 1: about 120 src lines (member set ~40, discovery ~30, `requires:` ~15, resolver ~35), the
  index skill (~40 lines of Markdown), and the grid skill deleted (-72).
- Stage 2: no Rust; the gate script is about 100 lines of Python plus its selfcheck.
- Prompt: +~100 bytes, only when ripwire is found.
- Disk: +258,093 bytes per ripwire install (the 16 skills).

## Open questions

1. **Card and index skill both?** The 18-line card stays in every ripwire session because the
   automatic layers' notices quote its commands. Folding it into the index skill would save about
   1 KB of prompt but leave the model unaware of the commands until it reads the skill. The plan
   keeps the card.
2. **Do `python/skills` count?** They are kernel library skills, not tools. The plan reads their
   `covers:` (so `goal` can cover a tool later) but lists none of them as major.
3. **Name collisions** with a user's own `ripwire-*` skill resolve to the user's, silently. The
   alternative is to name the shadowing on the index skill's read.

## D-rows owed

- Plugin skills: a plugin row may publish skill directories from its verified archive; they are
  readable as `yi://skills/<name>` and never listed. A Yi skill with `requires:` points at them.
- Skill coverage: every model tool and every plugin is covered by a skill (`covers:`) or pending
  on a shrink-only list; a skill covering what no longer exists fails.

## Not building

- Rewriting or editing ripwire's skills. They ship as released, pinned by checksum.
- `ripwire-mcp`. Yi runs no MCP server for ripwire.
- Ripwire's hooks or `install.sh`.
- A skill per feature-ledger row; the owner chose model tools and plugins.
- Auto-attaching any plugin skill, or a classifier trigger for them. They are reached by the
  pointer only.

## Self-review (rev 2)

The rev 1 text above was checked against the code. The findings, most severe first:

1. **Deleting `skills/yi/grid` does not remove it from anyone's machine.** `yi update`'s
   `stage_skills` (`crates/cli/src/update.rs:558`) copies the old `~/.yi/skills` tree, then writes
   the release's files over it, so a skill the release dropped stays. The owner's own
   `~/.yi/skills/yi/grid` is live in the catalog today. Only `just install-skills` removes it,
   because it uses `rsync --delete`. Fix: the release owns `~/.yi/skills/yi/` as a unit, and
   `update` swaps that directory whole, as `install-skills` does. Skills the user wrote stay outside
   `yi/`. Without this, the principle's "delete the feature, delete the skill" holds only in the
   repository.
2. **The gate's tool list is incomplete.** `tool_surface.json` has no `tool:ask_user`, yet
   `ask_user` is a model tool with its own guide (`fetch/docs.rs:13`); it is conditional, so the
   surface lock never sees it. Fix: enforce with a T0 Rust test in yi-runtime that builds the real
   tool registry and the plugin table, rather than a Python gate parsing a baseline and Rust source
   by regex. That test reads `skills/**/SKILL.md` and `baselines/skill_pending.json`, and fails on
   a gap, a covered pending row, or a `covers:` with no target.
3. **The plan ignored a primitive that exists.** `yi://tools/<name>` already serves on-demand
   guides for `edit`, `plan`, `todo` and `ask_user` (`TOOL_GUIDES`). Rev 1 invented `covers:`
   beside it. Either a guide counts as the tool's skill, or guides become skills, with one noun
   and one address. This is the owner's call (open question 4).
4. **The principle can produce duplicate prose.** A `read` or `bash` skill that restates the tool
   description adds nothing, and the duplication gate (production duplication 0, `.md` included)
   would refuse it. Fix: the rule names a content bar. A tool's skill teaches what the description
   cannot: when to use it over another tool, how calls combine, and recovering from its failures.
   It never restates the schema.
5. **A hot-path cost.** Rev 1 made `Plugin::installed` require every listed `SKILL.md`, but
   `search_path` calls `installed` on every spawn, which would mean 17 `stat`s per command. Fix: the
   member set is published by one rename, so the version directory existing proves completeness.
   No binary-only installs exist outside this machine, since #1142 is unmerged. Land stage 1 on
   #1142's branch and drop the check.
6. **The hand-written index drifts.** A pin bump that renames or adds a skill leaves the index
   table stale. Fix: a test asserts the index's addresses equal `Plugin::skills` exactly, and the
   count leaves the description.
7. **A PATH-only ripwire gets a pointer to nothing.** `requires: ripwire` was "found", which a
   Homebrew copy satisfies with no skills behind it. Fix: `requires:` means the plugin's skills
   directory exists, which is to say installed by `yi plugin add`. A PATH copy keeps the card only.
8. **The skills ship unread.** They were written for Claude Code: they name `Read`, `Bash`, the
   Skill tool and hooks. Some verbs write (`--note-add`) or clone (`ripwire <git-url>`), which Yi's
   read-only bash allowlist does not pass, so the model would hit permission asks the skill does not
   expect. Fix: stage 1 runs the `yi-dogfood` pass, every command each skill tells the model to run
   put through `yi gate`, and the index marks the skills whose commands write or ask.
9. **No one reviews 258 KB of third-party instructions on a pin bump.** The archive digest proves
   the bytes, not their intent. Fix: `baselines/plugin_skills.json` holds a sha256 per skill file,
   so a bump's PR shows exactly which skills changed, and the reviewer reads those.
10. **Unpacking a member set widens the archive surface.** Rev 1's install takes one member by exact
    name; a prefix set must also refuse `..`, absolute paths and non-regular entries, and cap the
    count and bytes it unpacks.
11. **Children never see the index.** A child's catalog lists no home skills (§7.8), so the worker
    children that do the editing will not see it. This is accepted for now and listed as open
    question 5.

### Revised build order

| Stage | Change from rev 1 |
|---|---|
| 0 | `yi update` swaps `~/.yi/skills/yi/` whole (finding 1). It is small and stands alone, and it lets the grid skill actually go. |
| 1 | On #1142's branch: the member set with a path guard and caps; no `SKILL.md` check in `installed`; `requires:` meaning skills present; the index skill plus a test that it equals `Plugin::skills`; `plugin_skills.json`; the dogfood pass and its notes in the index. |
| 2 | The coverage test in Rust over the real registry, not a Python gate; the `.ruler` rule with the content bar; the D-row. |
| 3 | Unchanged: one issue per remaining gap, after question 4 is settled. |

### Open questions added

4. Does an on-demand tool guide (`yi://tools/<name>`) count as the tool's skill? If yes, `edit`,
   `todo`, `plan` and `ask_user` are covered and pending holds `annotate`, `bash`, `get_context`,
   `grep`, `ipython`, `read` and `write`. If no, the guides and skills should merge into one noun.
5. Should a worker child see plugin index skills?
