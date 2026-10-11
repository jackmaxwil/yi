# Keys

`yi` starts a daemon (`yi serve`) on `~/.yi/daemon.sock` if none is listening and
opens the workspace over it. The daemon owns every session and its worker; the
console renders. Sessions keep running while no console is attached.

Two ways out:

| keys | what happens |
|---|---|
| `⌥q` (or `ctrl+b q`, `/quit`) | the console closes; the daemon and its sessions keep running |
| `ctrl+c` `ctrl+c` | the first press warns, the second stops the daemon and every worker, then quits |

A console attached to a daemon from an older build says so in the pane: restart it
with `ctrl+c` `ctrl+c` and run `yi` again.

## The workspace

The rail on the left lists sessions newest first: a slot number, the session's
avatar, its state, and in the full sidebar (`⌥b` cycles rail → full → hidden) its
name and age. Over kitty or Ghostty the avatar is an identicon drawn from the
session id; everywhere else it is the same two initials on the same colour. The
session in front wears a tinted row.

| keys | |
|---|---|
| `⌥1..9` / `⌘1..9` | resume that rail slot into the focused pane |
| `⌥n` | new session in the focused pane |
| `⌥v` `⌥s` `⌥x` `⌥z` | split right, split down, close, zoom |
| `⌥←→↑↓` | move focus between panes (solo child focus when there is one pane) |
| `⌥t` `⌥]` `⌥[` `ctrl+b 1..9` | tabs |
| `⌥/` | palette: `e path` opens an editor pane, `md path`, `diff path`, `nb` |
| `⌥⇧j` `⌥g` | notebook pane, diff pane for the focused session |
| `Tab` | sidebar ↔ panes |
| `ctrl+b` | prefix for terminals that eat alt |

A chat pane is solo's chat, the same code: composer with history and paste
markers, `/` verbs, `@` files, `esc esc` for the entry tree, `/plantree`, the
permission popup (`y` / `a` / `n`), the working line and orb, the status row.
