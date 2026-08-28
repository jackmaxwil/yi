---
name: session-mining
description: >
  Sweep past Yi session logs for repeated pain points — tool failure streaks,
  permission denials, retries, aborts, pivots — and produce a clustered,
  human-gated report. Use when asked to mine sessions, find agent pain
  points, analyze past runs, or on a scheduled heartbeat. Never applies
  fixes; it reports.
---

# Session mining

Yi sessions are typed JSONL under `~/.yi/sessions/` (one file per session,
one JSON object per line). They carry structured signals far stronger than
prose: tool results with error flags, permission denials with evidence,
retries, aborts, and advisory outcomes.

## Method: sweep in the kernel, judge in context

Do the bulk work as Python in the `ipython` kernel so thousands of entries
never enter model context — only the cluster table comes out.

1. **Sweep.** In one kernel program: walk `~/.yi/sessions/*.jsonl`, parse
   each line defensively (count and skip corrupt lines — never abort the
   sweep), and extract per session:
   - tool calls whose results carry errors, keyed by tool name and the
     shortest decisive error line;
   - streaks: the same tool failing 2+ times consecutively;
   - permission denials and holds, with their evidence text;
   - aborts and interrupted turns;
   - pivots: an error followed by the assistant switching tools or approach.
2. **Cluster.** Group by (tool, normalized error line). Keep counts, session
   ids, and one representative example per cluster. Sort by frequency.
3. **Report.** Print only the cluster table plus a coverage line — this is
   the honesty rule, never skip it:
   `N sessions scanned, M skipped (reason), covering DATE..DATE`.
4. **Propose, never apply.** For each top cluster, propose the smallest fix:
   a prompt or skill wording change, a triggered reminder rule, a missing
   tool affordance, or an upstream bug. Append the table and proposals to a
   pain-points ledger file when the user names one. Offer — behind explicit
   confirmation — `gh issue create` for items that belong in a tracker.
   Never edit config, rules, or skills yourself; the user lands changes.

## Backtesting a proposed rule

Before proposing a triggered reminder rule, replay its match over the same
swept sessions in the kernel: report how many times it would have fired and
where. A rule that would fire constantly is noise — tighten the trigger or
drop the proposal. Include the backtest count with the proposal.
