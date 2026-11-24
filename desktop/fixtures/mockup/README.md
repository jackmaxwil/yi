# Mockup fixtures

Everything here is pulled out of the built mockup page by script. Nothing is typed by hand, so nothing can drift from the page.
Do not edit these files; regenerate them.

```
python3 tools/fixtures/extract.py
```

Needs headless Chrome. `python3 tools/fixtures/extract.py --selfcheck` runs it twice into scratch folders and fails unless the two
runs are byte-identical and every table made the same number of rows. The page is read from
`~/Development/t3-rune-port/mockups/yi-desktop.html` (`--page` names another copy). The page is never changed.

## How it is read

The extractor appends a probe script to a copy of the page and loads `?solo=N&t=20` for each frame in headless Chrome. The probe
reads the page's own constants by name (`AG`, `LG`, `PL`, ...), scrapes the parts the page builds inline (settings, notifications,
menus, the findings hub), and walks the drawn elements for text and sizes. Rows that open in place are forced open first. Sizes
are `offsetHeight`, `offsetWidth` and computed styles, never client rects, because the page scales frames to fit.

## Folder

| Path | What |
|---|---|
| `data/<NAME>.json` | A table exactly as the page defines it, under the page's own name (`AG` agents, `SP` trace spans, `LG` log lines, `FD` findings, `AS` assets, `CTS` containers, `VMS`, `DMS` daemons, `DV` `DW` `DS` `DB` debugger lists, `LSV` `LSD` `DG` `DGD` language servers and problems, `PL` `PLD` plugins, `THEMES`, `CMX` right-click items by kind, `KIND` selector to kind, `G` sidebar, `T` threads, `R` `U` the file and the hunk, `UM` `A1` `A2` `TOOLS` the conversation) |
| `data/icons.json` | The icon helper's stroke settings and every icon's drawing data (24 by 24 view box) |
| `data/palette.json` | The page's state colours and its state table |
| `data/settings.json`, `notifications.json`, `status_menu.json`, `context_menu.json`, `hub.json`, `finding_detail.json`, `checks.json`, `tokens.json`, `agent_detail.json`, `debugger.json` | Parts the page builds inline, scraped from the drawn frame |
| `<slug>.rows.json` | The rows of a list pane in the shape `docs/conventions.md` gives. In a grouped list the group's first row also carries a `group.summary` text cell: the summary line the list engine draws beside the group name (the same text as `group_notes`) |
| `meta.json` | Per list pane: `open` (the row that is open in the mockup), `selected`, `group_notes` (the summary beside a group name, also written as the `group.summary` cell of the group's first row) |
| `panes/<slug>.text.json`, `panes/<slug>.sizes.json` | On-screen text and measured sizes of one pane, from the frame named below |
| `frames/<n>.strings.json`, `frames/<n>.sizes.json` | The same for a whole frame |
| `themes/<id>.json` | The five drawn themes in the convention's theme shape; token names follow the Edit theme pane |
| `intended.json` | What a scene may leave out or draw differently, each with its reason; read by `tools/parity/parity.py` |

## Cell values

`text`, `dot` (the page's own state names), `badge [text, tone]`, `pill [text, tone]`, `meter {ratio, text}`, `tags [..]`,
`bar {start, len, tone}` (fractions of the strip), `flag`, `series`. A coverage check cell is `flag true` (covered), `flag false`
(a gap) or `text "–"` (does not apply). Tones used: dim, faint, accent, need, ok, bad, warn, model. Where a detail label and a
column share a name with a different value, the detail's field gets the suffix `_detail`.

## Size parts

Each `sizes.json` entry is `{part, check, height, width, padding, font_size, line_height, selector}`. `check` lists the dimensions
parity compares. A scene tree is searched for the part by key: the key equals the part or ends with `.part`; `row` also matches any
key holding `.row.`, and `code-line` any key ending `.L<n>`. The keys a list pane draws, and so the parts that measure them:
`<pane key>.pane-header` (the pane's header bar), `<pane key>.table-header` (a table's header row), `<pane key>.footer`,
`<pane key>.row.<id>` (a row; a log line is a row) and `<pane key>.L<n>` (a code line). A part that no key matches fails parity
unless `intended.json` waives it with a reason, as `size: "<part>"` (or `"<part>.<dim>"` for each dimension it checks).


## Fields per list pane

Field names are the pane's column names in lower case with `_` between words; every fact of the opened detail sits beside them under its own label.

### `agents` (6 rows)

`agent` (text), `state` (dot), `doing_now` (text), `model` (badge), `tokens` (text), `cost` (text), `time` (text), `last_14_min` (bar), `asked_to` (text), `said_last` (text), `touched` (tags), `budget` (meter), `actions` (tags)

### `trace` (11 rows)

`step` (text), `start` (text), `took` (text), `timeline` (bar), `asked` (text), `steps` (text), `agents` (text), `cost` (text), `tokens` (text), `changed` (text), `actions` (tags), `model` (text), `read` (text), `wrote` (text), `first_word` (text), `decided` (text), `tool` (text), `file` (text), `got` (text), `looked_for` (text), `speed` (text), `made` (text), `change` (text), `agent` (text), `state` (text), `children` (text), `command` (text), `waiting` (text), `blocks` (text), `runs_in` (text), `takes` (text)

### `logs` (15 rows)

`time` (text), `level` (badge), `source` (text), `message` (text), `thread` (text), `turn` (text), `process` (text), `json` (text), `actions` (tags), `agent` (text), `service` (text), `host` (text), `request` (text), `in_trace` (text), `app` (text), `seen_via` (text), `container` (text), `image` (text), `peer` (text), `pool` (text), `seen` (text)

### `findings` (9 rows)

`severity` (badge), `finding` (text), `meta` (tags), `actions` (tags), `where` (text), `who_has_it` (text), `age` (text), `why` (text), `proof` (text), `proof_state` (text), `evidence` (tags), `group.summary` (text), `state` (pill), `blocks` (text), `seen_series` (series)

### `findings-table` (7 rows)

`severity` (badge), `finding` (text), `where` (text), `state` (pill), `who_has_it` (text), `age` (text), `why` (text), `proof` (text), `proof_state` (text), `evidence` (tags), `actions` (tags)

### `assets` (11 rows)

`state` (dot), `asset` (text), `about` (text), `reach` (flag), `logs` (flag), `backup` (flag), `scan` (flag), `seen` (text), `reach_detail` (text), `logs_detail` (text), `backup_detail` (text), `scan_detail` (text), `findings` (text), `actions` (tags), `group.summary` (text), `note` (text)

### `containers` (8 rows)

`state` (dot), `name` (text), `image` (text), `cpu` (meter), `mem` (text), `ports` (text), `up` (text), `health` (text), `memory` (text), `restarts` (text), `last_line` (text), `note` (text), `actions` (tags), `group.summary` (text)

### `vms` (6 rows)

`state` (dot), `name` (text), `cpu` (text), `memory` (meter), `disk` (text), `address` (text), `snaps` (text), `up` (text), `host` (text), `system` (text), `memory_detail` (text), `snapshots` (text), `note` (text), `actions` (tags)

### `daemons` (7 rows)

`state` (dot), `name` (text), `what_it_is` (text), `pid` (text), `up` (text), `restarts` (text), `login` (flag), `at_login` (text), `last_line` (text), `actions` (tags), `note` (text)

### `lsp` (3 rows)

`state` (dot), `server` (text), `language` (text), `project` (text), `memory` (text), `status` (text), `can_do` (tags), `agent_may_ask` (flag), `actions` (tags)

### `problems` (4 rows)

`severity` (badge), `message` (text), `where` (text), `why` (text), `fix` (text), `from` (text), `actions` (tags)

### `plugins` (9 rows)

`letter` (text), `color` (text), `name` (text), `version` (text), `enabled` (flag), `adds` (tags), `update` (flag), `needs_attention` (flag), `description` (text), `source` (text), `allowed` (tags), `denied` (tags), `actions` (tags)

### `settings` (10 rows)

`setting` (text), `description` (text), `key` (text), `value` (text), `set_by` (text), `changed` (flag)

### `themes` (5 rows)

`theme` (text), `note` (text), `swatches` (tags)

### `notifications` (4 rows)

`title` (text), `age` (text), `text` (text), `kind` (dot), `actions` (tags)

### `sidebar` (19 rows)

`title` (text), `state` (dot), `age` (text), `group.summary` (text)

### `debugger-variables` (5 rows)

`name` (text), `value` (text), `type` (text), `changed` (flag), `holds` (text), `lives_at` (text), `actions` (tags), `now` (text), `was` (text), `changed_at` (text), `which_is` (text), `passed_in_by` (text), `state` (text)

### `debugger-watch` (2 rows)

`expression` (text), `value` (text), `type` (text), `was` (text), `actions` (tags), `set_at` (text)

### `debugger-stack` (5 rows)

`frame` (text), `function` (text), `at` (text), `current` (flag), `variables` (text), `called_with` (text), `actions` (tags), `from` (text)

### `debugger-breakpoints` (3 rows)

`enabled` (flag), `where` (text), `condition` (text), `hits` (text), `stops` (text), `hit` (text), `added` (text), `actions` (tags), `stops_when` (text), `state` (text)

## Text and sizes: which frame

Each pane's text and sizes come from the one frame that draws it. Where a pane is drawn in several frames, the frame whose version carries the most of it is taken.

| Pane | Frame |
|---|---|
| `shell` | 14, Right-click (C1) |
| `shell-folded` | 1, Full layout (L1) |
| `chat-doc` | 17, Chat (CH3) |
| `new-chat` | 19, New chat (CH0) |
| `notifications` | 4, Notifications (N1) |
| `menu-bar` | 5, Mac menu bar with status menus (M1) |
| `context-menu` | 14, Right-click (C1) |
| `context-tray` | 23, Context boxes (X5) |
| `design` | 24, Design mode (X6) |
| `agents` | 6, Watching the work (P1) |
| `trace` | 6, Watching the work (P1) |
| `logs` | 6, Watching the work (P1) |
| `findings-table` | 6, Watching the work (P1) |
| `assets` | 7, Running things (P2) |
| `containers` | 7, Running things (P2) |
| `vms` | 7, Running things (P2) |
| `daemons` | 7, Running things (P2) |
| `debugger` | 8, Debugger and find references (P3) |
| `references` | 8, Debugger and find references (P3) |
| `lsp` | 9, Language servers and plugins (P4) |
| `problems` | 9, Language servers and plugins (P4) |
| `lsp-code` | 9, Language servers and plugins (P4) |
| `plugins` | 9, Language servers and plugins (P4) |
| `findings` | 10, Findings (F1) |
| `findings-gate` | 10, Findings (F1) |
| `findings-place` | 10, Findings (F1) |
| `findings-checks` | 10, Findings (F1) |
| `settings-groups` | 11, Settings (S1) |
| `settings` | 11, Settings (S1) |
| `settings-text` | 11, Settings (S1) |
| `themes` | 12, Themes (T1) |
| `theme-tokens` | 12, Themes (T1) |
| `plugin-kit` | 13, Yi builds a pane (K1) |
| `files` | 20, File tree (T7) |
| `chat` | 14, Right-click (C1) |
| `changes` | 14, Right-click (C1) |
| `code` | 1, Full layout (L1) |
| `terminal` | 14, Right-click (C1) |
| `web` | 1, Full layout (L1) |
| `desktop` | 1, Full layout (L1) |
| `report` | 1, Full layout (L1) |

Panes with text and sizes but no rows file: `shell`, `shell-folded`, `chat-doc`, `new-chat`, `menu-bar`, `context-menu`, `context-tray`, `design`, `debugger`, `references`, `lsp-code`, `findings-gate`, `findings-place`, `findings-checks`, `settings-groups`, `settings-text`, `theme-tokens`, `plugin-kit`, `files`, `chat`, `changes`, `code`, `terminal`, `web`, `desktop`, `report`.
