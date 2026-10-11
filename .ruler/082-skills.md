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
