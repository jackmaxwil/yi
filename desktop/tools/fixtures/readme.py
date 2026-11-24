"""Writes fixtures/mockup/README.md from what extract.py just produced, so the field lists cannot drift."""

FRAME_NAMES = {
    1: "Full layout (L1)", 2: "Any pane becomes a window (W1)", 3: "Two screens (W2)", 4: "Notifications (N1)",
    5: "Mac menu bar with status menus (M1)", 6: "Watching the work (P1)", 7: "Running things (P2)",
    8: "Debugger and find references (P3)", 9: "Language servers and plugins (P4)", 10: "Findings (F1)", 11: "Settings (S1)",
    12: "Themes (T1)", 13: "Yi builds a pane (K1)", 14: "Right-click (C1)", 17: "Chat (CH3)", 19: "New chat (CH0)",
    20: "File tree (T7)", 22: "Weight (X2)", 23: "Context boxes (X5)", 24: "Design mode (X6)",
}

HEAD = """# Mockup fixtures

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

"""


def kind_of(cell):
    return next(iter(cell)) if isinstance(cell, dict) else "?"


def render(built, pane_frames, panes, frames):
    out = [HEAD, "## Fields per list pane\n", "Field names are the pane's column names in lower case with `_` between words; every fact of the opened detail sits beside them under its own label.\n"]
    for slug, r in built.items():
        fields = {}
        for row in r.rows:
            for k, v in row["cells"].items():
                fields.setdefault(k, kind_of(v))
        out.append(f"### `{slug}` ({len(r.rows)} rows)\n")
        out.append(", ".join(f"`{k}` ({v})" for k, v in fields.items()) + "\n")
    out.append("## Text and sizes: which frame\n")
    out.append("Each pane's text and sizes come from the one frame that draws it. Where a pane is drawn in several frames, the frame whose version carries the most of it is taken.\n")
    out.append("| Pane | Frame |\n|---|---|")
    for slug, frame, *_ in panes:
        out.append(f"| `{slug}` | {frame}, {FRAME_NAMES.get(frame, '')} |")
    bare = [p[0] for p in panes if p[0] not in built]
    out.append("\nPanes with text and sizes but no rows file: " + ", ".join(f"`{b}`" for b in bare) + ".\n")
    return "\n".join(out)
