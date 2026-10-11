---
name: bash
description: >
  How a shell command is priced before it runs, how to carry work past one
  call, how to mine a cut output without rerunning it, and what to do after
  each refusal, timeout or failing exit a bash result can carry. Use before
  a build, a test suite, a server or anything that may outlive its call, and
  when a bash result ends in a `next:` or `[…]` row. Do NOT use to read,
  search or change files: read, grep, edit and write do that, and the
  kernel (the ipython skill) holds anything with state.
covers: tool:bash
---

# bash

`bash` → price the command → split reads from execs → background the long run → mine the cut → recover by notice

## What a call costs

Two separate judgments read every command before it runs.

The bash tool sorts it into a read or an exec. A read is a command whose
every segment (split on `|`, `;`, `&&`, newlines) is an inspection verb
(`ls`, `cat`, `grep`, `rg`, `find`, the reporting half of git, a ripwire
query) with no `>` redirect (`2>/dev/null` excepted), backtick or `$(`.
Reads run beside the other reads in the same message; an exec runs alone,
in transcript order.

The permission check decides whether it runs, and how. In auto mode a
command whose every segment is known-safe runs as is, one with a segment it
does not know runs contained in the sandbox, and a destructive or network
segment asks. In ask mode every bash call asks.

So keep inspection pure and separate from work:

- `ls crates/x` and `git log -- crates/x` are reads; `cd crates/x && ls`
  and `git -C crates/x log` are execs, since `cd` and `-C` are not
  inspection verbs.
- A pipe into `sort`, `uniq`, `awk`, `jq`, `sed` or `xargs` makes the whole
  call an exec. `sed -n 10,40p f` is an exec where `read` with a range is not.
- A plain view of exactly one file (`cat`, `head`, `tail`, `sed`, `grep`,
  `rg`, no pipe or redirect) comes back under a `[path#TAG]` header, but only
  `read` numbers the lines an edit anchors on. Converters (`pdftotext`,
  `pandoc`, `textutil`, `soffice`) earn a pointer back to `read`, which does
  that conversion itself.

## Work longer than one call

Size the call before the first run, not after a timeout: a cold build or a
whole suite gets `timeout_secs` near 600 and a `wait` of 30-60, so it turns
into a job and the turn is yours for reads in the meantime. Every timeout
spends its whole limit and returns nothing usable.

Past 600 s, or for anything meant to keep running (a dev server, a watcher),
detach it and read its log in later calls:

```
nohup ./serve --port 8123 > serve.log 2>&1 & echo $!
```

A contained server is reachable on 127.0.0.1 only. Keep the printed pid to
stop it. From the kernel, `rlm.bash` overlaps a long command with Python
work instead (the ipython skill).

## A cut output

A cut names `[full output: path]`. Point the `grep` tool's `path` at that
file for the failing lines, or `read` a range of it; never rerun a
five-minute build to see its middle. A log you will query more than once goes into the
kernel: open the path in a cell and filter it there.

## Recovery, by what the result says

| The result carries | Do |
|---|---|
| `[exit N inside a && chain: …]` | rerun from the failing segment; the ones after it never ran |
| `[timed out after Ns; pass timeout_secs …]` | raise it once and pass `wait`, or narrow the command |
| `[N of the last 4 bash calls hit their time limit; …]` | stop raising limits: shrink the input, sample, or `nohup` it |
| `[refused: … walks …]` | search from the cwd or name the directory; it never ran |
| `next: the sandbox refused writing …` | the code is not at fault; issue the call again when the write is needed, it asks and approval widens that run |
| `next: the sandbox refused this (…)`, naming a program that now needs permission | same: the next call using that program asks; do not rewrite it to dodge the network |
| `… nobody to answer refuses it` (no one at the keyboard) | report what needed the write or the network; do not retry |
| `[this output shows the sandbox refusing the network … though the command exited 0 …]` | exit 0 is not success here; nothing will ask, so tell the user what needs the network |
| `The user denied this call.` | take another approach; never reissue it |
| `… targets a protected path …; this is denied in every mode.` | no mode or approval passes it; drop that path |
| `Denied by the reviewer wall: …` | report the mismatch; the wall is fixed for the run |
| `[command aborted]` | Esc or the deadline stopped it; do not rerun it unasked |

A test that fails with PermissionDenied inside the sandbox is a fact about
where it ran, not about the code: the gate skill lists those tests.
