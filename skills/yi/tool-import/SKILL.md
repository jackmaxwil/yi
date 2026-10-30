---
name: tool-import
description: >
  Turn a project's just recipes into exec tools Yi can call, one wrapper each,
  so a build, test or status recipe is a tool with a schema instead of a bash
  call the model has to spell. Use when a repository has a justfile and the
  work keeps repeating `just <recipe>`; use it before writing a hand-made
  script under ~/.yi/tools.
trigger: justfile, just recipe, import just, exec tool, .yi/tools
within: crates/cli/src/tool_import.rs
---

# tool-import

`yi tool import just` → one wrapper per recipe → the schema it publishes → the
markers it reads → what breaks and what to say.

## What it writes

`yi tool import just [recipe ...]` dumps the justfile with just's own
`just --dump --dump-format json` and writes one executable wrapper per recipe
into `~/.yi/tools`, the directory `session_tools` scans at session build. A
running session keeps the table it started with, so **a newly imported tool
reaches the model at the next session, not this one**: tell the user that
rather than calling a tool the run does not hold.

`yi tool import just build test` imports only the recipes named. Naming one
that the justfile no longer has, or that is private or marked `yi: skip`, is a
refusal that says which.

## The schema the model reads

Each wrapper answers `--schema` with the recipe's name, its doc comment as the
description, and its parameters as an object: a parameter without a default is
`required`, one with a default carries `"default"` and is optional, and a
variadic (`*args`, `+args`) is an array of strings (`+` also required). A call
reaches the recipe as an argv array — one element per parameter, in order, the
variadic spread — never through a shell string, so a space or a quote in a
value is data.

## Kind, and the markers

A recipe is a `read` tool when its doc line is `yi: read` or its name starts
with a read-shaped word (`list`, `ls`, `show`, `status`, `info`, `log`, `diff`,
`cat`); otherwise it is `exec`. `yi: skip` and `[private]` keep a recipe out.
just keeps only a recipe's *last* comment line as its doc, so a marker must sit
directly above the recipe with nothing after it; a recipe that also wants a
description written differently takes `[doc('...')]`, which the dump then
reports instead — and the marker line is then invisible to the importer, so
pick one or the other.

## Rerun it, do not clean up by hand

The second line of every wrapper is `# yi: {"justfile":..., "recipe":...}`.
A rerun compares each wrapper against what it would write now (`added`,
`updated`, `unchanged`) and deletes the ones it wrote for recipes the justfile
dropped; a file with no marker is never deleted or overwritten, and one holding
a name it wants is a refusal naming the file. Deleting `~/.yi/tools/<name>` by
hand is safe and permanent — the next import puts it back.

## When it refuses

A recipe named like a built-in tool (`read`, `grep`, `bash`, `plan`, `todo`, …)
is refused by name before anything is written: rename the recipe, or import the
others and leave that one to `bash`. A non-zero exit is the refusal's own
sentence, so quote it to the user instead of retrying.

If a call fails after the import, the wrapper's error is just's: the recipe's
own exit code, its recipe line, and whatever the recipe printed to stderr. A
recipe edited since the import still runs — the wrapper names the justfile, not
the recipe text — but a *parameter* added since then is not in the schema, so
rerun the import.
