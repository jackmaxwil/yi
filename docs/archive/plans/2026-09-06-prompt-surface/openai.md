# OpenAI Codex system-prompt patterns — research report

Corpus root: `ref/prompts/system_prompts_leaks/OpenAI/` (abbreviated `C/` = `Codex/`, `C/old/` = `Codex/old/`).
All line numbers are from `cat -n` of the named file. Files fully read: codex-full.md (structural
sections + builtin tools + multi_agent + node_repl; MCP schema namespaces skimmed by header only),
gpt-6-astra.md, gpt-5.6.md, gpt-5.5.md, gpt-5.4.md, gpt-5.4-mini (diffed), gpt-5.3-codex-spark.md,
plan_mode.md, codex-auto-review.md, four personality files, old/gpt-5.md, old/gpt-5.1.md (diff),
old/gpt-5.2.md (diff), old/gpt-5-codex.md, old/gpt-5.1-codex-max.md, old/gpt-5.2-codex.md,
old/gpt-5.3-codex.md, computer-use.md, control-chrome.md, control-in-app-browser.md (head),
codex-desktop-realtime-voice-agent.md (head), README.md, gpt-5.5-api.md, gpt-5.6-sol.md (lines 60-107 + headers).

Corrections to the brief: there is no `API/README.md` (the `API/` dir holds only per-model
files); `C/codex-auto-review.md` is byte-identical to `C/gpt-5.4.md` (`cmp` returned equal), so
"auto-review" is not a separate prompt — it is the 5.4 base prompt with the pragmatic/friendly
personality slot (personality_pragmatic.md:4 lists `codex-auto-review` as a user). The
"Sandbox and approvals" section referenced at old/gpt-5.md:19 is not present in any capture; the
only sandbox text is the runtime-injected `<permissions instructions>` block.

---

## 1. STRUCTURE MAP of `C/codex-full.md` (359,565 bytes, 11,103 lines; captured from Codex Desktop 0.140.0-alpha.2, model gpt-5.5, reasoning xhigh)

| # | Lines | Bytes | Section | Static/dynamic | Notes |
|---|---|---|---|---|---|
| 1 | 1-141 | 19,790 | `# SYSTEM INSTRUCTIONS` — model prompt | static template; `{{ personality }}` slot at :5 | Byte-identical to `C/gpt-5.5.md` 1-138. Sub-sections: General :7, Engineering judgment :13, Frontend guidance :23-55, Editing constraints :57, Special user requests :72, Autonomy :77, Working with the user :82, Formatting rules :94, Final answer :113, Intermediary updates :127 |
| 2 | 142-147 | 391 | `<DEVELOPER_INSTRUCTIONS>` → `<permissions instructions>` | dynamic per session | Sandbox + approval policy text (see §2.1) |
| 3 | 149-191 | 4,192 | `<app-context>` Codex desktop | dynamic per client | Markdown image/mermaid rules :154-160, automations :165, thread tools :168-171, `::code-comment{}` directive :173-181, git directives `::git-commit{}` etc :182-190 |
| 4 | 193-207 | 970 | `<collaboration_mode>` Default | dynamic (Default vs Plan) | "mode changes only by developer message" :197; `request_user_input` availability :201-206 |
| 5 | 209-215 | 647 | `<apps_instructions>` | dynamic | `[$app](app://id)` mention syntax; lazy-load via `tool_search` |
| 6 | 217-253 | 4,010 | `<skills_instructions>` | dynamic (roots + catalog) | Skill roots alias table r0..r11 :220-232; catalog redacted :233; usage protocol :235-252 |
| 7 | 255-267 | 1,316 | `<plugins_instructions>` | dynamic | plugin = bundle of skills+MCP+apps |
| 8 | 269-402 | 6,874 | `## Memory` | dynamic (MEMORY_SUMMARY inlined :386-390) | decision boundary :273-283, layout :285-295, quick pass :297-309, verify-vs-drift matrix :319-339, citation block spec :341-379, update-only-via-notes :381-388 |
| 9 | 404-412 | 118 | `<USER_INSTRUCTIONS><INSTRUCTIONS>` | dynamic | This is where AGENTS.md content is injected (redacted) |
| 10 | 414-445 | 964 | `<ENVIRONMENT_CONTEXT>` | dynamic | key/value block: originator, cli_version, model, reasoning_effort, personality, collaboration_mode, approval_policy, sandbox_policy, permission_profile, cwd, workspace_roots, git.branch/commit |
| 11 | 447-602 | 3,336 | `<BUILTIN_TOOLS>` | static shape, client-injected | TS-style namespaces: `exec_command` :462-472, `write_stdin` :474, `update_plan` :492-498, `request_user_input` :500-510, `view_image` :516, `get_goal/create_goal/update_goal` :522-532, `apply_patch` FREEFORM grammar :534-560, `tool_search` :571, `multi_tool_use.parallel` :579 |
| 12 | 604-622 | 955 | `<TOOLS>` preamble | dynamic | 238 lazy tools across 12 namespaces |
| 13 | 623-1168 | 17,358 | `codex_app` (12 tools) | dynamic | thread mgmt, automations |
| 14 | 1169-1401 | 12,822 | `multi_agent_v1` (5 tools) | dynamic | delegation doctrine :1281-1311 lives inside the `spawn_agent` description |
| 15 | 1402-4829 | 95,256 | `mcp__codex_apps__github` (89) | MCP | JSON schemas verbatim |
| 16 | 4830-5697 | 34,085 | gmail (21) | MCP | |
| 17 | 5698-6576 | 21,280 | google_calendar (12) | MCP | |
| 18 | 6577-8211 | 59,365 | google_drive (35) | MCP | |
| 19 | 8212-8320 | 3,766 | openai_platform (3) + api_key_local_confirmation (1) | MCP | two-step key creation with local confirm gate |
| 20 | 8321-9007 | 16,806 | playwright (23) | MCP | |
| 21 | 9008-9880 | 23,363 | chrome_devtools (29) | MCP | |
| 22 | 9881-11041 | 26,177 | datascienceWidgets (5) | MCP | |
| 23 | 11042-11103 | 5,704 | node_repl (3) | MCP | 4 KB single-paragraph tool description :11046 |

Proportions: ~5.5% is the model prompt; ~6% developer/skills/memory scaffolding; ~1% builtin tool
shapes; ~87% lazy-loaded MCP schemas that are *not* in context until `tool_search` pulls them
(`defer_loading: true` on every entry; `load_workspace_dependencies` and `read_thread_terminal` are
the only `defer_loading: false` app tools, :994, :1042).

Where the tool schemas sit: builtin tool *shapes* are injected by the client, not stored in the
rollout (:449); they carry no descriptions in this capture. The descriptive layer for builtins
lives in the model prompt (rg, parallel, apply_patch rules) rather than on the tool.

AGENTS.md handling: in this capture it is an opaque `<USER_INSTRUCTIONS>` block (:404-412). The
spec text exists only in the older base prompts — `C/old/gpt-5.md:29-40` (scope = directory tree
rooted at the file; every touched file obeys every AGENTS.md whose scope includes it; deeper file
wins; direct system/developer/user instructions beat AGENTS.md; root + cwd-chain files are
pre-injected, subdirectory/outside-cwd files must be checked manually). By 6-astra the concern
flips to *over*-compliance: "Do not treat exceptions to requirements in local markdown and skill
files as automatically requiring user approval" (`C/gpt-6-astra.md:25`), and "user's instruction
... must take precedence over any guidelines provided in skills or external files" (:7).

Sandbox/approval text (verbatim shape, `C/codex-full.md:144-147`):
> Filesystem sandboxing defines which files can be read or written. `sandbox_mode` is
> `danger-full-access`: No filesystem sandboxing - all commands are permitted. Network access is
> enabled. Approval policy is currently never. Do not provide the `sandbox_permissions` for any
> reason, commands will be rejected.

The same facts are repeated as data in ENVIRONMENT_CONTEXT (:436-438) and as an operational note
on the tool shape (:451). Three redundant encodings of one policy = belt-and-braces for a fact the
model must never get wrong.

---

## 2. MECHANISMS catalog

### 2.1 Sandbox modes + approval policy
- What: `sandbox_mode` (e.g. `danger-full-access`; other modes implied: read-only, workspace-write)
  and `approval_policy` ∈ {never, on-failure, untrusted, on-request} (`C/old/gpt-5.md:172-175`).
- Tool surface: `exec_command{cmd, justification?, sandbox_permissions?: "use_default"|"require_escalated", prefix_rule?, tty?, workdir?, yield_time_ms?, max_output_tokens?}` (`C/codex-full.md:462-472`); `write_stdin{session_id, chars}` for interactive sessions (:474-479).
- Behavioral coupling: approval mode changes *testing* policy — "When running in non-interactive
  approval modes like never or on-failure, proactively run tests, lint"; "in interactive modes like
  untrusted or on-request, hold off on running tests or lint commands until the user is ready"
  (`C/old/gpt-5.md:172-175`; 5.1 softened "proactively" to "you can proactively ... If you are unable
  to run tests, you must still do your utmost best", `C/old/gpt-5.1.md:186`).
- Problem solved: expensive validation loops in approval-prompting modes; escalation requests
  the harness will reject.

### 2.2 AGENTS.md discovery
See §1. Precedence ladder (`C/old/gpt-5.md:34-39`): nested > root; prompt > AGENTS.md. Pre-injected
set: root + directories from CWD up to root. Later versions add the anti-over-compliance rule
(`C/gpt-6-astra.md:25`) and demand *citation* when a file causes a pause: "explicitly explain why
you need the confirmation (for example, a SKILL.md, AGENTS.md, memory, or approval auto-review
block) and where it came from" (`C/gpt-6-astra.md:13`).

### 2.3 Plan tool (`update_plan`) — checklist, not mode
- Shape: `update_plan{explanation?, plan: [{step, status: pending|in_progress|completed}]}` (`C/codex-full.md:492-498`).
- Spec (`C/old/gpt-5.md:279-287`): steps are 1-sentence, 5-7 words; "exactly one `in_progress` step
  until everything is done"; can mark several complete in one call; must end with all completed.
- Doctrine (`C/old/gpt-5.md:64-133`): plans "are not for padding out simple work with filler steps";
  "Do not repeat the full contents of the plan after an `update_plan` call — the harness already
  displays it" (:70); when-to-use list :74-82 (multi-action, dependencies, ambiguity, checkpoints,
  multiple asks, user said TODOs, generated extra steps); three good + three bad example plans
  :84-133 (bad = vague 3-liners like "Make styles look good").
- 5.1 hardening (`C/old/gpt-5.1.md:85-86`): "Do not jump an item from pending to completed: always
  set it to in_progress first. Do not batch-complete multiple items after the fact ... Do not let
  the plan go stale while coding." → symptom: models were retro-filling plans.
- Codex-tuned models compress to three rules (`C/old/gpt-5-codex.md:33-38`): skip for "roughly the
  easiest 25%"; "Do not make single-step plans"; update after each sub-task.
- 5.3-codex onward drops the Plan tool section entirely; 5.5 retains one line: "update item
  statuses incrementally as each item is completed rather than marking every item done only at the
  end" (`C/gpt-5.5.md:136`). Plan Mode (§7) is a separate mode; `update_plan` errors inside it
  (`C/plan_mode.md:15`).

### 2.4 `apply_patch` format
- Grammar (`C/codex-full.md:534-560`): `*** Begin Patch` / `*** End Patch`; hunks `*** Add File:`,
  `*** Delete File:`, `*** Update File:` (+ optional `*** Move to:`); context lines `@@` or `@@ text`;
  change lines prefixed `+`/`-`/space; `*** End of File` marker.
- Prose spec + example (`C/old/gpt-5.1.md:298-336`), guards: "NEVER try `applypatch` or
  `apply-patch`, only `apply_patch`" (`C/old/gpt-5.md:144`); "This is a FREEFORM tool, so do not wrap
  the patch in JSON" (`C/old/gpt-5.1.md:157`); "You must prefix new lines with `+` even when creating
  a new file" (:336); "Do not waste tokens by re-reading files after calling `apply_patch` on them.
  The tool call will fail if it didn't work" (`C/old/gpt-5.md:155`).
- Policy drift: 5-codex "Try to use apply_patch for single file edits, but it is fine to explore
  other options" (`C/old/gpt-5-codex.md:23`) → 5.4 "Always use apply_patch for manual code edits.
  Do not use cat or any other commands" (`C/gpt-5.4.md:15`) → 5.5 adds "Do not use Python to read
  or write files when a simple shell command or `apply_patch` is enough" (`C/gpt-5.5.md:60`).
  Exemptions preserved throughout: formatters, bulk mechanical rewrites.

### 2.5 Personality layers (three stacked)
1. Base-model API layer (`gpt-5.5-api.md:8-16`): "Desired oververbosity for the final answer (not
   analysis): 1 (low), 3 (medium), 7 (high)" on a 1-10 scale, "treated only as a default"; valid
   channels `analysis, commentary, final`; "Juice: 0/16/48/128/768" (reasoning budget by effort).
   ChatGPT layer adds a banned "verbal tic" list ("My honest recommendation", "Honestly? ...",
   `gpt-5.6-sol.md:82-84`).
2. `{{ personality }}` slot (5.2-codex → 5.5): `personality_pragmatic` (values Clarity/Pragmatism/
   Rigor; "avoid cheerleading, motivational language, artificial reassurance"; "don't comment on
   user requests, positively or negatively", `C/personality_pragmatic.md:14-26`) and
   `personality_friendly` ("we"/"let's", "never make the user work for you", "avoid open-ended
   questions, prefer a list of options", `C/personality_friendly.md:22-27`). The 5.5 friendly
   variant is rewritten as an "inner life" essay (`C/personality_friendly_gpt-5.5.md:12-20`).
3. Baked-in (5.6, 6-astra): `# Personality` section in the prompt itself (`C/gpt-5.6.md:3-21`,
   `C/gpt-6-astra.md:27-57`) with an explicit anti-slop word list: "delve," "foster," "leverage,"
   "it's worth noting," "genuinely", "Bottom Line:", contrastive "X, not Y" framing, "invented
   compound labels like 'exact-head checks'" (`C/gpt-6-astra.md:41-43`).
- Problem solved: model-specific tics. 5.5 needed "Never talk about goblins, gremlins, raccoons,
  trolls, ogres, pigeons" (`C/gpt-5.5.md:123,131`) and "do not lean on words like 'seam', 'cut', or
  'safe-cut'" (:116) — both are symptom lines for a specific checkpoint.

### 2.6 Preambles / progress updates (commentary channel)
- Origin (`C/old/gpt-5.md:43-62`): preamble before tool calls, "no more than 1-2 sentences ...
  (8-12 words for quick updates)", group related actions, skip for trivial single `cat`, eight
  example strings.
- 5.1 "User Updates Spec" (`C/old/gpt-5.1.md:48-63`): heads-down note before long stretches;
  "Before the first tool call, give a quick plan with goal, constraints, next steps"; say so when
  the plan changes.
- Cadence by version: 5.3-codex "every 20s" (`C/old/gpt-5.3-codex.md:93`); 5.4/5.5 "every 30s"
  (`C/gpt-5.4.md:101`, `C/gpt-5.5.md:132`); spark "every 3-5 tool calls" (`C/gpt-5.3-codex-spark.md:108`);
  5.6/6-astra "should not be left without a commentary update for more than 60 seconds"
  (`C/gpt-5.6.md:37`, `C/gpt-6-astra.md:77`).
- Content rules: "Before performing file edits of any kind, you provide updates explaining what
  edits you are making" (`C/gpt-5.5.md:137`); "interrupt your thinking and send multiple updates in
  a row if thinking for more than 100 words" (`C/gpt-5.4.md:106`); vary sentence openers (:102);
  "Never praise your plan by contrasting it with an implied worse alternative" (`C/gpt-5.5.md:130`).
- Channel discipline (5.6+): "Do NOT put a final response (e.g. a blocking / clarifying question)
  in the commentary channel"; "The final answer must always be fully self-contained: users should
  never need to read earlier commentary updates, since they are collapsed" (`C/gpt-5.6.md:39`).

### 2.7 Final-answer formatting spec — see §4.

### 2.8 Skills
- Catalog form (`C/codex-full.md:217-253`): name + description + short path with alias roots
  (`r0`..`r11`). Protocol: trigger on `$Name` or description match, "must use that skill for that
  turn"; "Do not carry skills across turns unless re-mentioned" (:238); read `SKILL.md` "completely
  ... If a read is truncated or paginated, continue until EOF" (:242); "Do not delegate reading,
  summarizing, or interpreting skill instructions to a subagent" (:244); prefer `scripts/` and
  `assets/` over retyping (:245-246); "choose the minimal set ... state the order" (:248);
  context hygiene: "Avoid deep reference-chasing" (:251).
- 5.6 adds announcement rules: tell the user *why* in commentary before an unnamed skill fires;
  mention material influence only in the final (`C/gpt-5.6.md:161-167`).
- 6-astra adds anti-false-trigger: "Do not use a skill based solely on keywords, superficial
  relevance, or the availability of a potentially applicable skill" (`C/gpt-6-astra.md:144`);
  stale-path recovery (:142); quote-the-instruction when a skill causes a pause (:138).

### 2.9 MCP / apps / plugins / tool_search
- Every non-builtin tool is `defer_loading: true` and pulled via `tool_search{query, limit?}`
  (`C/codex-full.md:571-576`). Apps are `[$name](app://id)` mentions; "Do not additionally call
  list_mcp_resources ... for apps" (:214). Plugins are bundles, "not invoked directly" (:264).
- Tool descriptions carry their own routing prose (see §5) — e.g. `create_thread` "only when the
  user explicitly asks" (:709), `spawn_agent` "Only use ... if and only if the user explicitly asks
  for sub-agents" (:1281).

### 2.10 Compaction ("compact" instructions)
- 5.5: "When you run out of context, the tool automatically compacts the conversation. That means
  time never runs out ... Do not restart from scratch" (`C/gpt-5.5.md:90`); post-resume sanity check
  "answering the newest request, not an older ghost" (:88).
- 5.6: "you will see all prior user requests. Assume the last user request is current and previous
  requests are stale but useful context"; "Do not redo completely finished work or repeat already
  delivered commentary updates; treat a turn spanning compactions as one logical chain" (`C/gpt-5.6.md:31`).
- 6-astra reverses the "last = current" default: "Treat the most recent user message as the latest
  steering for the active task, not automatically as a replacement objective ... preserve the
  original objective, accepted corrections, current constraints, completed work, and outstanding
  work" (`C/gpt-6-astra.md:69`).

### 2.11 Review mode
- Universal paragraph, every version from 5-codex on (`C/old/gpt-5-codex.md:43`, `C/gpt-5.5.md:73`):
  "prioritize bugs, risks, behavioral regressions, and missing tests. Findings should lead the
  response ... ordered by severity and grounded in file/line references; then add open questions
  or assumptions; then include a change summary as secondary context. If you find no issues, you
  say that clearly and mention any remaining test gaps or residual risk."
- Desktop inline-comment directive: `::code-comment{title, body, file, start?, end?, priority 0-3}`
  "Emit one directive per inline comment; emit none when there are no actionable inline comments"
  (`C/codex-full.md:173-181`); the example carries a `[P2]` severity prefix in the title.
- 5.6 request-type table: "Answer, explain, review, or report status ... do not authorize external
  writes" (`C/gpt-5.6.md:97`).

### 2.12 Spark / fast mode
`C/gpt-5.3-codex-spark.md:1`: "your sampling speed is 1.5k tokens per second ... every tool call
(no matter how simple) is expensive and slow. The user would prefer that you make mistakes rather
than over-explore ... NEVER run useless commands like `echo X`." STRICT ONE_SHOT MODE (:51-59):
identify files, "Read each required file at most once per task", "apply changes in a single
patch/application phase", only re-read on hard failure. Validation bans (:61-71): NEVER re-check,
review, list, re-read, use git, run tests; "HARD STOP ... You WILL lose 100 points"; "If you
realize you put a bug in the code, tell the user rather than going back and correcting your bug".
Frontend from scratch: "do NOT explore the codebase or read files" (:42). This is the extreme end
of the cost-of-tool-call dial; see §8.

### 2.13 Memory (desktop)
`C/codex-full.md:269-402`: skip/use decision list with hard-skip examples (:273-283); layered
files general→specific (:285-295); "Quick-pass budget: ideally <= 4-6 search steps before main
work" (:313); verification matrix (drift-prone & cheap → verify; drift-prone & expensive → answer
but flag "memory-derived, may be stale"; low-drift & expensive → answer directly, :321-329);
mandatory `<oai-mem-citation>` trailer with `file:start-end|note=[...]` entries and rollout ids
(:341-379); memory is written only through "one small file" in `extensions/ad_hoc/notes/` and
only "when explicitly asked by the user" (:381-388).

### 2.14 Multi-agent (`multi_agent_v1`)
Gate: "Only use `spawn_agent` if and only if the user explicitly asks ... Requests for depth,
thoroughness, research ... do not count as permission" (`C/codex-full.md:1281-1283`). Doctrine
:1285-1311: plan first, "explicitly decide what immediate task you should do locally right now";
delegate "concrete, bounded sidecar tasks"; never delegate the critical-path blocker; "disjoint
write set" per worker; "Call wait_agent very sparingly"; "Do not repeatedly wait by reflex".
Roles in the schema (:1316): `explorer` ("fast and authoritative ... trust the explorer results
without additional verification"), `worker` (assign file ownership; "tell workers they are not
alone in the codebase, and they should not revert the edits made by others").

### 2.15 Goal tools
`get_goal`, `create_goal{objective, token_budget?}`, `update_goal{status: complete|blocked}`
(`C/codex-full.md:522-532`). No prose in this capture; shape only. Relevant to Yi's goal module.

### 2.16 Structured questions
`request_user_input{questions:[{header, id, question, options:[{label, description}]}]}`
(`C/codex-full.md:500-510`). Rule: "Never write a multiple choice question as a textual assistant
message" (:205); tool only "when it is listed in the available tools for this turn" (:203).
6-astra adds timing: "30 seconds for a simple multi-choice question ... before proceeding with a
stated assumption. If an answer or approval is required, keep the question pending ... Elapsed
time is not an answer or approval" (`C/gpt-6-astra.md:65`).

### 2.17 Destructive-action protocol (5.6+)
`C/gpt-5.6.md:114-131`: resolve exact targets with read-only checks; "Do not use `$HOME`, `~`, `/`,
a workspace root ... as the target of a recursive or destructive command"; `mktemp -d`; "Never
repurpose `$HOME`, `$home`, or `$CODEX_HOME`" (:83, :124 — repeated twice, so it happened);
"Prefer recoverable operations, such as moving files to trash"; "After deleting anything
material, briefly tell the user what was removed and whether it can be recovered".

### 2.18 Desktop side-channel directives
`::git-stage{cwd}`, `::git-commit{cwd}`, `::git-create-branch{cwd branch}`, `::git-push{}`,
`::git-create-pr{... url isDraft}` — "Only emit these git directives in your final response after
the action actually succeeds, never in commentary updates" (`C/codex-full.md:182-190`). Same for
`::created-thread{threadId=...}` (:171). A cheap way to let the UI react to model-claimed events
without parsing prose.

---

## 3. WORKFLOW DOCTRINE

**Autonomy / keep going**
- gpt-5: "keep going until the query is completely resolved, before ending your turn" (`C/old/gpt-5.md:137`).
- 5.1: adds "persevere even when function calls fail" (`C/old/gpt-5.1.md:150`) and the standing
  paragraph reused through 5.5: "do not stop at analysis or partial fixes; carry changes through
  implementation, verification, and a clear explanation" (`C/old/gpt-5.1.md:42`).
- 5.5: "Do not end your turn while `exec_command` sessions needed for the user's request are still
  running" (`C/gpt-5.5.md:76`).
- 6-astra: "Do not stop at acknowledging capability (e.g. 'Yes…'), proposing a plan, or offering to
  continue" (`C/gpt-6-astra.md:21`); "Do not settle for a partial or 'helpful enough' solution ... to
  save time, effort or tokens" (:21).

**Intent inference (code vs. talk)**
- "Unless the user explicitly asks for a plan, asks a question about the code, is brainstorming
  ... assume the user wants you to make code changes" (`C/old/gpt-5.1.md:44`, unchanged to 5.5:78).
- 5.6 replaces the binary with a four-way dispatch: Answer/explain/review → "evidence-backed
  response ... do not authorize external writes"; Diagnose → "Do not implement the fix unless the
  user asks"; Change/build → implement, verify "in proportion to risk"; Monitor/wait → use the
  product mechanism (`C/gpt-5.6.md:95-100`).
- 6-astra: "'can you...', 'I want to...', 'help me...' ... treat these as instructions to do the
  work" (`C/gpt-6-astra.md:21`).

**When to ask**
- Default: "strongly prefer making reasonable assumptions and executing" (`C/codex-full.md:205`).
- 5.6 boundary: bias to action when "read-only ... or impacts only the systems, data, and people
  the user placed in scope" or "a normal implementation step within the requested workflow"
  (`C/gpt-5.6.md:102-104`); stop when "completion requires new authority, external coordination, or
  a meaningful expansion beyond the user's implied intent" (:112); "A terminal condition such as
  'finish,' 'babysit,' or 'do not stop' requires persistence ... but does not broaden the set of
  authorized actions" (:106).
- 6-astra: "The user gets very frustrated when you stop and ask" (`C/gpt-6-astra.md:13`); "do all
  the work first so that user approval is the final step" (:9); "ask the user for clarification
  while continuing independent work" (:23); authorization persists across turns (:7).

**Verification / testing**
- gpt-5: "start as specific as possible to the code you changed ... then make your way to broader
  tests"; "do not add tests to codebases with no tests"; formatter "iterate up to 3 times"; "If
  the codebase does not have a formatter configured, do not add one" (`C/old/gpt-5.md:163-167`).
- 5.5: "let test coverage scale with risk and blast radius" (`C/gpt-5.5.md:19`); Three.js must be
  verified "with Playwright screenshots and canvas-pixel checks" (:43).
- 6-astra: "Do not write tests for reversible, low-impact changes or that mirror the
  implementation"; "Once those pass, broaden or repeat testing only when new changes, failures, or
  unresolved concerns justify it" (`C/gpt-6-astra.md:125-126`) — anti-over-testing, the opposite
  of the Yi doctrine, worth noting as a frontier-model correction.
- Honesty: "If you weren't able to do something, for example run tests, you tell the user"
  (`C/gpt-5.5.md:120`).

**Scope discipline**
- "Fix the problem at the root cause rather than applying surface-level patches" (`C/old/gpt-5.md:148`).
- "Do not attempt to fix unrelated bugs or broken tests. It is not your responsibility" (:150, :169).
- "Ambition vs. precision": greenfield → "feel free to be ambitious"; existing codebase → "surgical
  precision ... don't overstep (i.e. changing filenames or variables unnecessarily)" (:177-183).
- 5.5 "Engineering judgment" block: "prefer the repo's existing patterns"; "structured APIs or
  parsers instead of ad hoc string manipulation"; "add an abstraction only when it removes real
  complexity" (`C/gpt-5.5.md:15-18`).
- 6-astra: "Do not introduce unsolicited warnings, disclaimers, approval flows, or safety/
  compliance checklists due to hypothetical risk" (`C/gpt-6-astra.md:123`).

**Git hygiene**
- "NEVER revert existing changes you did not make" (`C/old/gpt-5-codex.md:25`); "Do not amend a
  commit unless explicitly requested" (:29, dropped in 5.5); "NEVER use destructive commands like
  `git reset --hard` or `git checkout --`" (:31); "You struggle using the git interactive console.
  ALWAYS prefer non-interactive git" (`C/old/gpt-5.2-codex.md:68`; 5.5 softens to "You are clumsy"
  :68); "Do not `git commit` your changes or create new git branches unless explicitly requested"
  (`C/old/gpt-5.md:156`); desktop: branch prefix `codex/` (`C/codex-full.md:183`).
- Unexpected changes escalation drift: 5-codex "STOP IMMEDIATELY and ask" (:30) → 5.4 "If they
  directly conflict ... stop and ask. Otherwise, focus" (`C/gpt-5.4.md:23`) → 5.5 "you do NOT
  revert them ... Only ask ... if those changes make the task impossible" (`C/gpt-5.5.md:66`).
- 6-astra PR-body craft: "write the exact text to a temporary file and pass it with --body-file"
  (`C/gpt-6-astra.md:119`); "`JSON.stringify()` is not shell escaping" (:122).

**What "done" means**
- 5.5: implementation + verification + "a clear account of the outcome" (`C/gpt-5.5.md:76`).
- 6-astra reporting contract: "explain what changed, why, how it was tested, and any material
  risks or limitations" (`C/gpt-6-astra.md:49`); "Summarize routine verification instead of listing
  every check" (:51); PR descriptions "for a reviewer who has not seen the conversation" (:57).
- 5.6: "hand off the completed result while a safe, relevant next step remains" (`C/gpt-5.6.md:99`).

---

## 4. FINAL-ANSWER FORMATTING spec (full extraction)

### 4.1 The canonical block — `C/old/gpt-5.md:205-268` (gpt-5 / 5.1 / 5.2 base prompts)
Preamble: "You are producing plain text that will later be styled by the CLI. Follow these rules
exactly. Formatting should make results easy to scan, but not feel mechanical." (:207)

**Section Headers** (:209-215)
- Optional; "not mandatory for every answer".
- 1-3 words, `**Title Case**`, "Always start headers with `**` and end with `**`".
- "Leave no blank line before the first bullet under a header."

**Bullets** (:217-223)
- "`-` followed by a space for every bullet."
- Merge related points; one line each; "short lists (4-6 bullets) ordered by importance".
- "consistent keyword phrasing and formatting across sections".

**Monospace** (:225-229)
- Backticks for commands, paths, env vars, code identifiers, inline examples, literal keyword bullets.
- "Never mix monospace and bold markers; choose one".

**File References** (:231-239)
- Include start line; "Use inline code to make file paths clickable"; "Each reference should have
  a stand alone path. Even if it's the same file"; accepted forms absolute / workspace-relative /
  `a/` `b/` / bare filename; `:line[:column]` or `#Lline[Ccolumn]`; no `file://`/`vscode://`; "Do
  not provide range of lines"; examples `src/app.ts:42`, `b/server/index.js#L10`,
  `C:\repo\project\main.rs:12:5`.

**Structure** (:241-248)
- Related bullets together; "general → specific → supporting info"; subsections start with a
  bolded keyword bullet; "Multi-part → headers and grouped bullets. Simple → minimal headers".

**Tone** (:250-256)
- "like a coding partner handing off work"; "no filler"; "present tense and active voice ('Runs
  tests' not 'This will run tests')"; "don't refer to 'above' or 'below'"; parallel structure.

**Don't** (:258-264)
- No literal words "bold"/"monospace"; "Don't nest bullets"; "Don't output ANSI escape codes";
  don't cram keywords; don't let keyword lists run long.

**Adaptation** (:266-268): code explanation → "precise, structured explanation with code
references"; simple implementation → "lead with the outcome"; large change → "logical
walkthrough ... rationale ... next actions"; casual → "respond naturally without section headers".

**Length** (:203): "very concise (i.e. no more than 10 lines), but can relax this requirement for
tasks where additional detail and comprehensiveness is important".

### 4.2 The 5.1 verbosity table — `C/old/gpt-5.1.md:271-277` ("enforced")
- Tiny/small single-file change (≤ ~10 lines): "2-5 sentences or ≤3 bullets. No headings. 0-1
  short snippet (≤3 lines) only if essential."
- Medium (single area / few files): "≤6 bullets or 6-10 sentences. At most 1-2 short snippets
  total (≤8 lines each)."
- Large/multi-file: "Summarize per file with 1-2 bullets; avoid inlining code ... (still ≤2 short
  snippets total)."
- "Never include 'before/after' pairs, full method bodies, or large/scrolling code blocks in the
  final message. Prefer referencing file/symbol names instead."
(Dropped in 5.2, replaced by softer prose; but this is the sharpest length-cap table in the corpus.)

### 4.3 Codex-model compression — `C/old/gpt-5-codex.md:45-80`
Same rules as one bullet each ("Bullets: use - ; merge related points; keep to one line when
possible; 4-6 per list ordered by importance"). Adds: "Don't dump large files you've written;
reference paths only"; "When suggesting multiple options, use numeric lists ... so the user can
quickly respond with a single number" (:59); "Do not start this explanation with 'summary', just
jump right in" (:57).

### 4.4 5.2-codex → 5.4 rules — `C/old/gpt-5.2-codex.md:22-38`, `C/gpt-5.4.md:58-92`
- "If the task is simple, your answer should be a one-liner."
- "Never use nested bullets ... if you use : just include the line you might usually render using
  a nested bullet immediately after it. For numbered lists, only use the `1. 2. 3.` style markers
  (with a period), never `1)`." (repeated twice in 5.4: :62 and :91 — reinforcement by duplication)
- Headers: `**…**`, "Don't add a blank line."
- File links become markdown links with absolute targets: `[app.py](/abs/path/app.py:12)`; spaces
  → `[My Report.md](</abs/path/My Project/My Report.md:3>)`; "Do not wrap markdown links in
  backticks ... This confuses the markdown renderer"; "Avoid repeating the same filename multiple
  times when one grouping is clearer" (`C/gpt-5.4.md:66-72`).
- "Don't use emojis or em dashes unless explicitly instructed." (:73)
- 5.4 prose-first turn (:77-92): "For simple or single-file tasks, prefer 1-2 short paragraphs
  plus an optional short verification line. Do not default to bullets"; "if there are only one or
  two concrete changes you should almost always keep the close-out fully in prose"; larger tasks
  "at most 2-3 high-level sections"; "grouping by major change area or user-facing outcome, not by
  file or edit inventory"; "If the answer starts turning into a changelog, compress it: cut
  file-by-file detail, repeated framing, low-signal recap, and optional follow-up ideas before
  cutting outcome, verification, or real risks"; "Use lists only when the content is inherently
  list-shaped"; no openers ("Done —", "Got it", "Great question", "You're right to call that
  out"); hard cap "Never overwhelm the user with answers that are over 50-70 lines long".

### 4.5 5.5 additions — `C/gpt-5.5.md:94-123`
- Nested-bullet ban gains an exemption: "This does not apply to generated artifacts such as PR
  descriptions, release notes, changelogs, or user-requested docs" (:98).
- "never end your answer with an 'If you want' sentence" (:115).
- Prose register: "plain, idiomatic engineering prose with some life in it ... avoid coined
  metaphors, internal jargon, slash-heavy noun stacks, and over-hyphenated compounds" (:116).

### 4.6 5.6 / 6-astra — formatting rules shrink to rendering facts
- 5.6 keeps only file-link rules + CommonMark: "a blank line before any list ... a blank line
  between a header and any content ... required for correct rendering" (`C/gpt-5.6.md:15`, `C/gpt-6-astra.md:100`).
- "Lead with the outcome rather than the steps you took" (`C/gpt-5.6.md:19`).
- 6-astra: "Avoid section headings, and do not use concluding summary statements such as 'In
  short:..'" (`C/gpt-6-astra.md:35`); "Default to using clear, concise paragraphs, each developing
  one main idea" (:39).
- Visualization gate (`C/gpt-5.6.md:60-74`): use only for "several exact mappings", "one source ...
  affecting three or more downstream consumers", "three or more dependent steps", hierarchy, or
  hard-to-linearize bugs; "smallest useful visual: a table ... a flow or timeline ... a tree ... a
  wireframe"; "Compact notation and small examples do not count as visualizations".

Trend: rules that were *typographic* for a CLI renderer (headers-in-bold, no blank line) migrate
to *shape* rules (prose vs list, lead with outcome, 50-70 line cap) and then to *register* rules
(banned words, no contrastive framing) as the model gets stronger — the renderer facts stay,
the taste rules get rewritten each generation.

---

## 5. TOOL DESCRIPTION craft

**Shell (`exec_command`)** — no description on the shape in this capture (`C/codex-full.md:462-472`);
all shell guidance lives in the prompt:
- Command hints with reasons: "prefer using `rg` or `rg --files` respectively because `rg` is
  much faster than alternatives like `grep`. (If the `rg` command is not found, then use
  alternatives.)" (`C/old/gpt-5.md:276`) — hint + fallback in one line.
- Explicit list of parallelizable reads: "`cat`, `rg`, `sed`, `ls`, `git show`, `nl`, `wc`"
  (`C/gpt-5.4.md:9`); "Use `multi_tool_use.parallel` ... and only this".
- Negative example with reason: "Never chain together bash commands with separators like
  `echo "====";` as this renders to the user poorly" (`C/gpt-5.4.md:9`); "Do not use python scripts
  to attempt to output larger chunks of a file" (`C/old/gpt-5.md:277`).
- Injection guard: "backticks and `$()` passed to the `cmd` argument will still execute"
  (`C/gpt-5.6.md:81`); "Treat shell command text as code" (`C/gpt-6-astra.md:122`).
- Latency guard: "Avoid performing blocking sleep or wait calls longer than 60 seconds"
  (`C/gpt-5.6.md:82`).
- 6-astra moves to a JS exec tool: "Batch independent searches and reads in one functions.exec
  using await Promise.allSettled([...]); inspect every result. Keep dependencies, edits,
  approvals, waits, and adaptive follow-ups sequential" (`C/gpt-6-astra.md:115`).

**apply_patch** — freeform grammar on the tool (:534-560) + prose with a worked example covering
all three ops and a rename (`C/old/gpt-5.1.md:298-336`) + "It is important to remember:" two-item
checklist at the end (header required, `+` prefix on new-file lines). Guards against the two
observed failure modes: wrong tool name (`applypatch`), JSON-wrapping a freeform payload.

**update_plan** — shape (:492-498) + 3-sentence contract (5-7 words per step, one in_progress,
mark all complete) + when-to-use bullets + paired good/bad examples (`C/old/gpt-5.md:84-133`).
The good/bad pairing is the only place in the corpus with explicit *negative examples of output*.

**spawn_agent** — the longest builtin-adjacent description (:1271-1374): model roster with
efforts, a three-line hard gate, then four H3 sub-sections of doctrine *inside the tool
description*, then role definitions embedded in the `agent_type` enum description string
(:1316). Pattern: put usage doctrine on the tool so it only loads when the tool loads.

**node_repl.js** — 4 KB single paragraph (:11046) listing every helper (`nodeRepl.write`,
`emitImage`, `cwd`), every gotcha (`var` vs `const` redeclare, `process` blocked "because the
current Rust-server-to-Node-child transport runs over stdio"), and a `title` param "Short
user-facing description ... for example `Inspect package metadata`" (:11062). Pattern: a
user-facing `title` argument on exec tools gives the UI a label without parsing the command.

**Skill front-matter as tool routing** (`C/control-chrome.md:1-4`, `C/computer-use.md:1-4`): YAML
`name` + `description` written as trigger conditions ("Use for tasks that require ... Prefer a
dedicated plugin or skill when it can complete the task"). Body pattern: routing rules → named
sub-docs fetched on demand (`agent.documentation.get("confirmations")`, :16-27) → bootstrap code
→ "never mention Node REPL to the user" (:30) → safety block (:90-103). Confirmation taxonomy is a
numbered policy table with four tiers (hand-off / always confirm / pre-approval works / never
confirm, `C/computer-use.md:34-90`) plus hygiene rules ("Don't ask early: only confirm when the
next action will cause impact", :99).

---

## 6. VERSION EVOLUTION table

| Version (file) | Size | Added | Removed / rephrased | Implied problem |
|---|---|---|---|---|
| gpt-5 base (`C/old/gpt-5.md`) | 21 KB | AGENTS.md spec; preamble examples; planning doctrine + good/bad plans; task-execution rules (root cause, no unrelated fixes, no re-read after patch, no commits); validation ladder + approval-mode coupling; ambition vs precision; full typographic final-answer spec; ≤10 lines default; inline citation ban `【F:…】` | — | Model output ChatGPT-style citations; over-formatting; plan padding; re-reading files after edits |
| gpt-5.1 base (`C/old/gpt-5.1.md`) | 25 KB | "Autonomy and Persistence" section; User Updates Spec (heads-down notes, plan before first call); plan-state rules (no pending→completed jumps, no batch-complete, no stale plan); "persevere even when function calls fail"; apply_patch prose spec + "FREEFORM, do not wrap in JSON"; **Verbosity** cap table | preamble examples kept | Stopping at analysis; retro-filled plans; JSON-wrapped patches; long final messages with before/after code |
| gpt-5.2 base (`C/old/gpt-5.2.md`) | 22 KB | `multi_tool_use.parallel` rule; "beautiful and modern UI" one-liner | User Updates Spec and preamble examples deleted; "Sharing progress updates" deleted | Updates handled elsewhere (harness/commentary channel) |
| gpt-5-codex (`C/old/gpt-5-codex.md`) | 7 KB | Terse rewrite: Editing constraints (ASCII, sparse comments, dirty-worktree rules, no amend, STOP IMMEDIATELY on foreign changes, no reset --hard); 3-rule Plan tool (skip easiest 25%, no single-step); review mindset; numeric option lists | Everything tutorial-shaped gone (AGENTS spec, plan examples, validation ladder) | Codex-tuned checkpoints already know the workflow; prompt = constraints only. Model reverting user edits and amending commits |
| gpt-5.1-codex-max | 8 KB | Frontend "AI slop" block (fonts, no purple-on-white, motion, backgrounds) | — | Generic-looking UIs |
| gpt-5.2-codex | 8 KB | `{{ personality }}` slot; "You struggle using the git interactive console"; "Don't use emojis"; formatting moved to top | Review paragraph rewritten in second person | Interactive git hangs; emoji output |
| gpt-5.3-codex (`C/old/gpt-5.3-codex.md`) | 10.5 KB | commentary/final channels; Autonomy paragraph (from 5.1 base); parallel tool calls; "Do not use Python to read/write files"; markdown links for files; no em dashes; no interjection openers ("Done —", "Got it"); update cadence every 20s; "interrupt your thinking ... if thinking for more than 100 words" | Plan tool section dropped | Silent long thinking stretches; python heredoc file writes; em-dash tic |
| gpt-5.3-codex-spark | 12 KB | 1.5k tok/s framing; ONE_SHOT MODE; validation bans with "lose 100 points"; "prefer mistakes over over-exploration"; updates per 3-5 tool calls | — | Fast model burning latency on `ls -R`, re-reads, self-review |
| gpt-5.4 (`C/gpt-5.4.md`) | 13 KB | "expert coding agent" persona paragraph; "Always use apply_patch ... Do not use cat"; `echo "====";` chaining ban; softer foreign-change rule (only stop on conflict); React patterns; prose-first final answer (2-3 sections, changelog compression, 50-70 line cap); cadence every 30s; "You're right to call that out" added to banned openers | "no python" retained; "Don't use nested bullets" now stated twice | cat-heredoc edits; separator-noise in shell output; bullet-list changelogs; sycophantic openers |
| gpt-5.4-mini | 11 KB | — | Reverts to 5.3-style file-reference and final-answer bullets (diff shows the prose-first paragraphs and 50-70 cap absent) | Smaller model gets the simpler, more mechanical rules |
| gpt-5.5 (`C/gpt-5.5.md`) | 20 KB | Second-person "you" voice throughout; Engineering judgment block; 30-line Frontend design spec (lucide icons, 8px radius, no orbs, no cards-in-cards, Three.js + Playwright pixel checks, palette bans); dev-server handoff; "Do not end your turn while exec_command sessions ... still running"; newest-message-wins + post-compaction sanity check; compaction paragraph; prose register rules (no "seam"/"cut"); no "If you want" closers; "Never praise your plan by contrasting"; goblins/raccoons ban ×2; incremental checklist updates | "Do not amend a commit" dropped; "expert coding agent" persona replaced by "senior engineer's judgment ... through attention rather than premature certainty" | Creature metaphors and "seam/cut" jargon in a specific checkpoint; contrastive self-praise; ending turns with background processes; desktop app (not just CLI) audience |
| gpt-5.6 (`C/gpt-5.6.md`) | 18 KB | Personality baked in (curious, "another subjectivity"); Technical communication (lead with outcome, describe what tools did not their names); commentary ≤60s + "final must be self-contained"; Visualizations gate; shell-escaping/`$()` warning; 60s sleep cap; `$HOME` repurposing ban; four-way request-type dispatch (Answer/Diagnose/Change/Monitor); authorization boundary; "finish/babysit does not broaden authority"; Destructive actions section; Skills section moved into model prompt with announce rules | Frontend guidance gone; Engineering judgment gone; header/bullet typography rules gone (only CommonMark blank-line + file-link rules remain); 30s cadence → 60s; goblins line gone; `{{ personality }}` slot gone | Over-reach on "diagnose" requests; `rm -rf $HOME`-class incidents; command-substitution leaks; blocking questions lost in commentary |
| gpt-6-astra (`C/gpt-6-astra.md`) | 21 KB | "When to ask the user for permission" as *first* section; authorization persists across turns; "do all the work first so that user approval is the final step"; "user gets very frustrated when you stop"; must cite the SKILL.md/AGENTS.md that caused a pause; AI-slop word list; "state the intended action directly ... avoid adding what you won't do"; PR description craft; `request_user_input_async` with 30s wait semantics; steering-not-replacing message model; JS `functions.exec` with `Promise.allSettled`; `--body-file`; "JSON.stringify is not shell escaping"; anti-over-testing; skill trigger discipline (no keyword-only) | Destructive-actions section gone (moved to tooling?); request-type dispatch gone; "Do not use Python" gone | Frontier model over-asks permission, over-tests, over-hedges; AGENTS.md/skill text read as veto; slop vocabulary |

Cross-version constants (present in every file from gpt-5 to 6-astra): `rg` preference with
fallback; "the user does not see command execution outputs"; "never tell the user to save/copy
this file"; no `git reset --hard`; don't revert changes you didn't make; review = findings first
by severity with file:line.

---

## 7. PLAN MODE and AUTO-REVIEW methodology

**Plan Mode** (`C/plan_mode.md`, 8.8 KB, injected as `<collaboration_mode>` replacing Default):
- Contract: plan must be "decision complete, where the implementer does not need to make any
  decisions" (:3); mode is sticky — "If a user asks for execution while still in Plan Mode, treat
  it as a request to plan the execution" (:9); `update_plan` errors in this mode (:15).
- Mutation boundary (:17-39): allowed = reads, static analysis, dry runs, "Tests, builds, or checks
  that may write to caches or build artifacts ... so long as they do not edit repo-tracked files";
  forbidden = edits, formatters, patches, codegen; tiebreak "if the action would reasonably be
  described as 'doing the work' rather than 'planning the work,' do not do it".
- Three phases: (1) Ground — "Eliminate unknowns in the prompt by discovering facts, not by asking
  the user"; "at least one targeted non-mutating exploration pass" before any question (:41-49).
  (2) Intent chat — goal, success criteria, audience, scope, constraints, tradeoffs; "if any
  high-impact ambiguity remains, do NOT plan yet—ask" (:51-54). (3) Implementation chat —
  interfaces, data flow, edge cases, tests, rollout, migrations (:56-58).
- Question discipline (:60-90): use `request_user_input`; each question must "materially change
  the spec/plan, OR confirm/lock an assumption, OR choose between meaningful tradeoffs" and "not be
  answerable by non-mutating commands". Two unknown types: discoverable facts → explore, then
  "present concrete candidates (paths/service names) + recommend one"; preferences → "2-4
  mutually exclusive options + a recommended default. If unanswered, proceed with the recommended
  option and record it as an assumption".
- Output (:92-128): one `<proposed_plan>` block per turn, tags on their own lines, sections
  Summary / Key Changes / Test Plan / Assumptions; "Prefer grouped implementation bullets by
  subsystem or behavior over file-by-file inventories ... avoid naming more than 3 paths"; "do not
  invent detailed schema, validation, precedence, fallback, or wire-shape policy unless the
  request establishes it"; "Do not ask 'should I proceed?'"; revisions are complete replacements.

**Auto-review**: no distinct methodology prompt exists in the corpus — `codex-auto-review.md` is
the 5.4 prompt verbatim. The review method is the "Special user requests" paragraph (§2.11) plus
the desktop `::code-comment` directive with `priority` 0-3 and `[P2]`-style titles
(`C/codex-full.md:173-181`). 6-astra references "an approval auto-review block" that can reject
actions and must be reported: "explicitly tell the user that automatic approval review rejected
the action, identify the action, and summarize the stated reason. Put this explanation in a
short, separate paragraph at the end of both commentary and final" (`C/gpt-6-astra.md:13`) — i.e.
auto-review is a *gate on actions*, not only a code reviewer, and the model is told how to
surface its verdicts.

---

## 8. TOP 15 transferable ideas for Yi (cheap open-weight + frontier models)

1. **Constraints-only prompt for tuned models, tutorial prompt for general ones.** The codex-tuned
   prompts are 7 KB of rules; the base-model prompts are 21-25 KB with examples and rationale
   (`C/old/gpt-5-codex.md` vs `C/old/gpt-5.md`). Yi should ship two tiers keyed by model class: a
   glm-flash tier with worked examples and a frontier tier with constraints. The 5.4-mini diff
   shows OpenAI does exactly this (mechanical bullets for mini, prose-judgment for full).

2. **Command hint + reason + fallback in one line.** "prefer `rg` ... because `rg` is much faster
   ... (If the `rg` command is not found, then use alternatives.)" (`C/old/gpt-5.md:276`). Weak
   models follow hints that carry a reason and a fallback; bare "use rg" produces retries when rg
   is missing.

3. **Name the parallelizable set explicitly.** "`cat`, `rg`, `sed`, `ls`, `git show`, `nl`, `wc`"
   (`C/gpt-5.4.md:9`). A closed list beats "parallelize when possible" for a small model.

4. **Negative examples with the observable symptom.** `echo "====";` chaining "renders to the user
   poorly" (:9); `applypatch`/`apply-patch` name guard (`C/old/gpt-5.md:144`); "do not wrap the
   patch in JSON" (`C/old/gpt-5.1.md:157`). Each is a real failure the team saw; write Yi's from its
   own incident log (the CLAUDE.md "Incident:" grants are the same idea).

5. **Length caps by change size, as a table.** The 5.1 verbosity table (`C/old/gpt-5.1.md:271-277`)
   is the tightest formulation in the corpus: ≤10-line change → 2-5 sentences, no headings, ≤3-line
   snippet; medium → ≤6 bullets, ≤2 snippets ≤8 lines; large → 1-2 bullets per file. Plus the hard
   ceiling "50-70 lines" (`C/gpt-5.4.md:92`) and "never before/after pairs or full method bodies".

6. **"Do not re-read after apply_patch; the call fails if it didn't work"** (`C/old/gpt-5.md:155`).
   Cheap models waste 20-30% of tool calls on confirmation reads. Yi's hashline edit already
   returns success/failure — say so in the prompt and let the tool result carry the new hash
   context so the model never needs the re-read.

7. **Cost-of-tool-call dial.** Spark's ONE_SHOT MODE (`C/gpt-5.3-codex-spark.md:51-59`) is the far
   end; a milder version for glm-flash: "read each required file at most once", "plan edits after
   the first read pass, apply in one patch phase", "re-read only on hard failure". Pair with the
   "prefer mistakes over over-exploration" framing when the user is pairing synchronously.

8. **Preamble cadence in tool calls, not seconds, for small models.** Spark uses "every 3-5 tool
   calls" (:108) because a fast model has no wall-clock sense. Also: "Before performing file
   edits of any kind, you provide updates explaining what edits you are making"
   (`C/gpt-5.5.md:137`) — a one-line preamble before each edit is a cheap self-check that catches
   wrong-file edits in weak models.

9. **Plan tool as state machine with explicit forbidden transitions.** "Do not jump an item from
   pending to completed: always set it to in_progress first. Do not batch-complete multiple items
   after the fact" (`C/old/gpt-5.1.md:85`); "exactly one in_progress". Yi's plan module can *enforce*
   these as value-encoded refusals (the fixtures already pin done-on-pending as refusal) and the
   prompt can state them so the model does not fight the refusal.

10. **Good/bad plan pairs.** (`C/old/gpt-5.md:84-133`). The only output-level negative examples in
    the corpus; small models copy the shape of examples far more than they follow adjectives like
    "high quality". Include two pairs in the glm tier.

11. **Four-way request-type dispatch table.** Answer/Diagnose/Change/Monitor with per-row
    authorization (`C/gpt-5.6.md:95-100`). This is a "when X do Y" table that prevents the two
    classic weak-model failures (implementing when asked to diagnose; refusing to implement when
    asked to fix). Yi's D-row for state-space-as-data applies: encode as a table the runtime can
    show in the prompt.

12. **Review output contract.** Findings first, severity-ordered, file:line, then open questions,
    then summary; explicit "no findings + residual risk" sentence (`C/gpt-5.5.md:73`); optional
    machine-readable `::code-comment{... priority=N}` directive (`C/codex-full.md:173-181`). Yi can
    render the directive as TUI annotations without parsing prose.

13. **Success-only side-channel directives in the final message.** `::git-commit{cwd}` "only ...
    after the action actually succeeds, never in commentary" (`C/codex-full.md:182-190`). Cheap UI
    hooks; the "only after success" clause is the load-bearing guard for hallucinated completion.

14. **Compaction contract stated to the model.** "time never runs out"; "assume compaction
    occurred ... Do not restart from scratch"; "do not redo completely finished work or repeat
    already delivered commentary" (`C/gpt-5.6.md:31`); 6-astra's "most recent message = steering,
    not replacement" (`C/gpt-6-astra.md:69`). Yi's context manager should inject a one-paragraph
    equivalent at the compaction boundary — weak models otherwise restart exploration.

15. **Destructive-command checklist with concrete forbidden targets.** "Do not use `$HOME`, `~`, `/`,
    a workspace root ... as the target of a recursive or destructive command"; `mktemp -d`; "Never
    repurpose `$HOME`"; "avoid relying on unresolved environment variables, globs, or command
    substitutions to identify destructive targets" (`C/gpt-5.6.md:118-129`). Concrete token lists
    are what a small model can pattern-match; Yi's wall can enforce the same list and the prompt
    should name it so denials are not surprising.

Honourable mentions: `title` argument on exec tools for UI labels (`C/codex-full.md:11062`);
tool doctrine embedded in the tool description so it loads lazily (`spawn_agent`, :1281-1311);
skill "announce which skill and why, one line" + "quote the instruction if it makes you pause"
(`C/gpt-6-astra.md:138`); "Never write a multiple choice question as a textual assistant message"
(`C/codex-full.md:205`) — structured questions only via the tool; explicit banned-opener list
("Done —", "Got it", "Great question", "You're right to call that out", `C/gpt-5.4.md:86`) and
banned-closer ("If you want", `C/gpt-5.5.md:115`).

Anti-patterns to *not* copy: the goblins/raccoons line and the "seam/cut" ban are checkpoint-
specific tics, not doctrine; 6-astra's anti-over-testing ("Do not write tests for reversible,
low-impact changes", `C/gpt-6-astra.md:125`) contradicts Yi's testing doctrine and is a
frontier-model correction, not a small-model need; the 30-line frontend design spec
(`C/gpt-5.5.md:23-55`) is product-specific taste.
