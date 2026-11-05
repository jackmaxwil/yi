---
name: tool-import
description: "just recipes as tools: `yi tool import just`; next session"
trigger: justfile, just recipe, import just, exec tool, .yi/tools
---

# tool-import

An imported tool is registered at the next session's build, never in the run
that imported it: the tool table is fixed when a session starts. So import,
tell the user the tools arrive next session, and reach for one then. Do this
when a repository has a justfile and the work keeps repeating `just <recipe>`,
before writing a hand-made script under `~/.yi/tools`.

## What Yi sees

```
yi tool import just [recipe ...]        # no names: every recipe of the justfile

imported 3 just recipes from /p/justfile into /h/.yi/tools
  added 3: build, deploy, test        # also: updated, unchanged, removed,
                                      # left out (skipped)
exit 0 imported      exit 2 refused, one sentence on stderr: quote it, never retry

<recipe> --schema
{"name":"build","description":"<the recipe's doc line>","kind":"exec",
 "input_schema":{"type":"object",
   "properties":{"target":{"type":"string","default":"release"},
                 "targets":{"type":"array","items":{"type":"string"}}},
   "required":["value"]}}
```

A parameter without a default is `required`; one with a default carries it and
is optional; `*`/`+` is an array of strings (`+` also required). `kind` is
always `exec`: a repository-authored doc line cannot grant read-only
permission. A call hands the wrapper one JSON object on stdin and the wrapper
turns it into `just --justfile <file> <recipe> <argv…>`: one element per
parameter in order, a variadic spread one element each, an omitted literal
default passed as its default, and an omitted computed default left for just
to fill. Once a computed default is omitted, no later value can be supplied;
the wrapper refuses one rather than shifting it into the wrong parameter. A
required parameter the call omits is refused by name.

The marker is the recipe's doc line, which is the comment directly above it
with nothing between: `yi: skip` keeps the recipe out, and `[private]` does the
same. A project opts in by having a justfile with that doc line; there is no
config key.

## What it writes

`just --dump --dump-format json` is the only reader of the recipes, and each
one becomes a `/bin/sh` wrapper in `~/.yi/tools`, the directory `session_tools`
scans. A recipe with no parameters execs `just` itself; one with parameters
maps the call's JSON through `python3`, and the wrapper names `python3` and
exits 127 when it is missing.

`yi tool import just build test` imports only the recipes named, and prunes
nothing. Naming a recipe the justfile dropped, or one that is private or
marked `yi: skip`, is a refusal that says which.

## Rerun it, do not clean up by hand

The second line of every wrapper is `# yi: {"justfile":…, "recipe":…}`. A rerun
compares each wrapper against what it would write now (`added`, `updated`,
`unchanged`) and deletes the ones it wrote for recipes the justfile dropped; a
file with no marker, or one that is not text, is never deleted or overwritten,
and one holding a name it wants is a refusal naming the file. A name held by
another justfile's wrapper refuses the whole import before any write.
Deleting `~/.yi/tools/<name>` by hand is safe and permanent.

## When it refuses

A recipe named like a built-in tool (`read`, `grep`, `bash`, `plan`, `todo`, …)
is refused by name before anything is written: rename the recipe, or import the
others and leave that one to `bash`.

If a call fails after the import, the wrapper's error is just's: the recipe's
own exit code, its recipe line, and whatever the recipe printed to stderr. A
recipe edited since the import still runs — the wrapper names the justfile, not
the recipe text — but a parameter added since then is not in the schema, so
rerun the import.
