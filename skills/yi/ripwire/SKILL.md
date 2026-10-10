---
name: ripwire
description: Which ripwire skill fits this moment, each read on demand
requires: ripwire
covers: plugin:ripwire
---

# Ripwire's skills

Ripwire is the code map behind the post-edit `[ripwire check]` row and `get_context`'s task and
symbol layers. Its own skills teach the verbs for one moment each. Read the one that fits with
`read("yi://skills/<name>")`; a file a skill names beside itself is
`read("yi://skills/<name>/<file>")`.

| Moment | Skill |
|---|---|
| Unsure which one fits; "where do I start" | `ripwire-router` |
| Cold in a repository or subsystem: map before reading files | `ripwire-orient` |
| You can name the symbol: callers, callees, a path, what breaks if it changes | `ripwire-navigate` |
| About to write one symbol or add a dependency: find what already exists | `ripwire-reuse-first` |
| Starting a feature: plan, API boundary and scope from the real structure | `ripwire-before-you-build` |
| A symptom (crash, wrong output, failing test) and no suspect | `ripwire-find-bug` |
| A measured performance problem (profile, benchmark) | `ripwire-perf-target` |
| Tests for code that has none; does a test reach its target | `ripwire-write-tests` |
| Quality of what you just wrote, before saying done | `ripwire-quality-bar` |
| Safety of a diff before push or review: blast radius, tests to run | `ripwire-change-check` |
| Risk in code you did not write: hotspots, dead code, clones | `ripwire-fresh-eyes` |
| Architecture health: cycles, layering, boundary rules | `ripwire-layers` |
| A call-graph question the fixed verbs cannot phrase | `ripwire-graph-query` |
| Untrusted input paths, or a skill or MCP file before installing it | `ripwire-security-scan` |
| Briefing someone else on a subsystem | `ripwire-handoff` |
| Clang optimization remarks while editing ripwire's own C++ (contributors only) | `ripwire-opt-remarks` |

## Reading them in Yi

- They were written for Claude Code and Codex. "the Read tool" and "Bash" are Yi's `read` and
  `bash`. Their setup steps (`skills/install.sh`, `--hook`, `ripwire wrap`, the MCP server) are
  for those agents: skip them, ripwire is already installed for you.
- Most commands run contained in Yi's sandbox without asking. These write: `--note-add`,
  `--quality-baseline`, `--quality-ack`, `--arch … --baseline`/`--baseline-update`, `--html`,
  `--export=…:FILE`, `--index-out`, the edit verbs (`--replace-symbol-body`, `--insert-*`,
  `--edit-plan --apply`) and `--run-trace`, which runs the command it is given. A git URL as the
  root clones it.
- `skill not found` on one of these means ripwire was found on PATH rather than installed by
  `yi plugin add ripwire`, which is what brings the skills.
