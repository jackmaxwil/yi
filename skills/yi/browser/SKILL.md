---
name: browser
description: >
  Where the console's browser pane gets its Chrome, and what to tell a user whose pane shows
  an error row instead of a page. Use when the user asks about the browser pane, a
  `[browser: …]` or `[installing chrome-headless-shell …]` row, or `yi plugin add chrome`.
  You cannot drive the pane: it shows the user a page and gives you no tool.
covers: plugin:chrome
---

# The browser pane's Chrome

The pane runs the first Chrome it finds: `browser.path` in `~/.yi/config.json`, then an
installed Google Chrome or Chromium (the macOS app, or `google-chrome`, `chromium`,
`chrome` on PATH), then the Chrome for Testing headless shell that `yi plugin add chrome`
pins under `~/.yi/plugins/chrome/`. With none of them, the pane runs that install itself
(about 100 MB) and says so in its row; a configured `browser.path` that is missing is never
replaced by a download.

## Recovering

| The pane says | Tell the user |
|---|---|
| `couldn't download chrome …` | the network refused it; `yi plugin add chrome` in a terminal retries with the full error |
| `… has sha256 …, yi pins …` | the file on disk is not the pinned build: `yi plugin remove chrome`, then `yi plugin add chrome` |
| `yi pins no chrome build for …` | no headless shell exists for that machine (Linux on arm64): install Chromium, or set `browser.path` |
| `Chrome closed its pipe` on Linux | the headless shell is missing system libraries: `ldd <program> \| grep 'not found'` names them, and `deb.deps` beside the program lists Debian's packages |
| `page not drawn: … kitty graphics …` | the terminal cannot show images: open the console in Ghostty or kitty |

The install is one `yi plugin add` like any other: `yi plugin list` shows whether it landed,
and `yi plugin remove chrome` frees the space once a real Chrome is installed.
