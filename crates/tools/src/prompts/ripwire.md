# ripwire

`ripwire` maps this repository's code: ranked symbols for a task, callers,
callees, the tests that reach a symbol, and stack traces mapped to source.

    ripwire . --for="<the task in plain words>" --json   # what to read first
    ripwire . --callers=commit_prose --json             # who calls it
    ripwire . --affected=commit_prose                   # test files that reach it
    ripwire . --from-trace=- < trace.txt                # frames to file:line, innermost first
    ripwire . --at=src/app.rs:197                       # the definition enclosing a line

A symbol is a bare name, `file:name` or `Type::name`. Calls are matched by
name: a row can be a same-named function elsewhere, every count is a floor,
and a zero means none found, never none exists. Uses of a type are invisible
to it; grep for those. Pass `--json` where a verb takes it: the XML answers
open with a long legend.

Its edit verbs are not the write path here; the edit tool is.
