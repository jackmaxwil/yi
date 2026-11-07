# A skill for every tool

The owner, 2026-10-10: "every major agent tool/feature needs a corresponding skill". Again
2026-10-30, reviewing #1891: "every new feature, tool, pattern, etc available to Yi, MUST BE TAUGHT
TO YI with a visible schema and skill. Skills are lazy loaded with a pointer but always a
representative description and compact info."

- **Major** is every tool a root session's model can call and every row of the plugin table
  (`yi_tools::plugin::PLUGINS`). Beyond those, a `yi` subcommand or flag the model may run, a config
  key, or a marker or file format it reads or writes is taught the same way, in a skill; no gate
  reads those kinds yet, so the reviewer enforces them.
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
- A feature with no tool schema carries its shape in the skill's opening lines: a subcommand's usage
  line and output shape, a config key's type and default, a marker's grammar. A tool's own schema is
  already visible to the model, so its skill points at it.
- The catalog line is the first 60 characters of `description` (`DESCRIPTION_CLIP`), so lead with
  what the feature is and when to reach for it. Under budget pressure the trailing skills fall to a
  bare-name list.
- The pull request body says where Yi is taught: the skill path and the catalog line as rendered.
- Skills load lazily: one catalog line each, the body on `read("yi://skills/<name>")`. A plugin's
  own skills sit behind one pointer skill and are never listed.
