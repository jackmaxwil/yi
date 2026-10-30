# Teach every feature to Yi

The owner, 2026-10-30, reviewing #1891: "every new feature, tool, pattern, etc available to Yi, MUST
BE TAUGHT TO YI with a visible schema and skill. Skills are lazy loaded with a pointer but always a
representative description and compact info."

- A **feature** is anything Yi can use: a tool, a plugin, a `yi` subcommand or flag the model may
  run, a config key, a marker or file format it reads or writes, a convention. If Yi can reach it,
  the change that adds it teaches it. A feature nobody taught is half-shipped.
- A **visible schema** is the exact shape Yi sees without reading source: the tool's JSON schema, a
  subcommand's usage line and output shape, a config key's type and default, a marker's grammar. It
  sits compactly in the skill's opening lines, so the body costs one read and the catalog nothing.
- A **skill** is a shipped `SKILL.md` whose `description` stands alone as the catalog line: what the
  feature is, when to reach for it, the one thing that surprises. The catalog always shows
  `name: description` with the pointer `read("yi://skills/<name>")`; the body loads lazily. A skill
  that falls to the bare-name list, or a description that needs the body to make sense, is a defect.
- The pull request body says where Yi is taught: the skill path and the catalog line as rendered.
  A feature-ledger row says it in a `taught:` clause beside `journey:`.
- Deleting a feature deletes its skill. Until the gate in #1906 reads every kind of feature, the
  reviewer enforces this section; a pull request that adds a feature without it is not done.

# A skill for every tool

The owner, 2026-10-10: "every major agent tool/feature needs a corresponding skill".

- **Major** is every tool a root session's model can call and every row of the plugin table
  (`yi_tools::plugin::PLUGINS`).
- **Covered** means a shipped `SKILL.md` (under `skills/` or `python/skills/`) whose frontmatter
  says `covers: tool:<name>` or `covers: plugin:<name>` (a comma list), or an on-demand tool guide
  served at `yi://tools/<name>`.
- `request_budget::every_model_tool_and_plugin_is_covered_by_a_skill_or_pending` enforces it. Its
  `PENDING` list holds the gaps that predate the rule and may only shrink: a covered entry fails
  until it is removed, and a `covers:` naming a tool or plugin that no longer exists fails too.
- A tool or plugin lands with its skill, and deleting one deletes its skill. `yi update` replaces
  the bundled skill directory whole, so a deletion reaches every install.
- The content bar: a skill teaches what the tool description cannot — when to reach for this tool
  over another, how its calls combine, and how to recover from its failures. It never restates
  the schema or the description; the duplication gate refuses that.
- Skills load lazily: one catalog line each, the body on `read("yi://skills/<name>")`. A plugin's
  own skills sit behind one pointer skill and are never listed.
