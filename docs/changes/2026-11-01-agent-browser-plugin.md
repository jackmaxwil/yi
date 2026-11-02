---
version: 0.872.0
decisions: D528
issue: Refs #1958
growth: +495 about 120 lines are browser.rs's test module and 60 its verb table; weighed `--allowed-domains` (cut, CDNs break under a list of one site), a filtering proxy (cut), a contained daemon (cut, it needs the unix socket ban lifted), and a `needs:` column on the plugin table (cut, one row)
raise: comments +33
raise: over-cap +1
raise: tests +571
raise: crate kernel +12
raise: crate permission +284
raise: crate runtime +42
raise: crate tools +158
decision: the agent-browser plugin runs its daemon outside the sandbox and its Chrome inside one, with Chrome's own sandbox off | a Chrome outside read a planted ~/.ssh key and any contained program reached it over its loopback debug port, so the agent-desktop shape was unsafe; the owner, 2026-11-01: "One yes, then web", accepting that a page breaking Chrome reads source code and that contained programs can use the web session while it lives | delete the row and the table
ledger=: `agent-browser` plugin | 17.1 | optional | M | M | `yi plugin add agent-browser` (macOS, `host` shape: never by `--all`, never in an eval container) also installs `chrome` and lets the model drive a headless page by bash; Chrome runs in a sandbox that reads no credential and writes only temp. The default session has loopback alone and asks nothing; `--session web` has the internet and one yes covers it for the session; what the daemon itself would do asks every time; real logins and a visible window are refused in every mode, as is every command until the pinned program, its config and its script all stand, so a copy on PATH is refused; page text returns inside the external-data fence.
---
`yi plugin add agent-browser` gives the model a browser by bash, with no new tool, and makes true the sentence agent-desktop's browser refusal already says. The obvious copy of agent-desktop was probed and refused: a Chrome outside the sandbox read a planted `~/.ssh` key, and a contained `curl` drove it over its loopback debug port to reach the web. So the daemon stays outside, since the sandbox bans its unix socket, and Chrome goes inside a sandbox of its own through a script `add` writes. The default session's Chrome has loopback alone, so a contained program reaching it gains nothing and nothing asks. `--session web` adds the internet by address, with one yes per session. A repo's `agent-browser.json` is never read, and the daemon goes after five idle minutes rather than an hour.
