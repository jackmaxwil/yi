---
name: checker
description: >
  Make a child that cannot see your work write the checks: a worker walled
  from history turns the task text into a program of PASS and FAIL lines, and
  you start each requirement row with the check that proves it. Use when a
  person will grade the result against checks you cannot read, or when you
  wrote both the work and its test. Do NOT use for a task whose acceptance is
  already a command; run that command.
---

# The checker

A run that writes its own definition of done grades itself. The wall between
the two is the point: the child that writes the checks reads the task, never
your attempt at it. Nothing here is hidden from the implementer either; the
checks are handed over with the work.

## The recipe

1. Write the requirement list first, as `unattended` does: one row per
   requirement of the task text, in the text's own words.
2. Spawn one worker walled from history, with the task text in its brief and
   nothing of this transcript. Every write it makes is outside the workspace
   you are editing:

```python
worker = await rlm.run(
    "From the task text in this brief alone, write <checks_dir>/checks.py: one "
    "`PASS <id>` or `FAIL <id>` line per requirement, runnable as python3 with "
    "no arguments from the workspace. Do not read the workspace's own tests and "
    "do not write a fix; a check that cannot run says why.",
    role="worker",
    deny_url=["history://"],
)
print(worker.name)
```

`deny_url=["history://"]` is the wall, not a preference. Walled, the child
reads no transcript of yours, of this session's parent, or of a sibling, so
its checks come from the instruction instead of from a copy of what you did.

3. Start every requirement row with the check that proves it, and step the row
   done only on that check's own exit or `PASS` line. In a plan, the row's
   `check=` is its contract's `decider: {cmd}` and the engine runs it; with no
   plan, run the checks program yourself before each row's `done`.
4. A verifier is a reader, and a reader may only append. It runs the checks on
   the finished work and adds the requirements the checks found missing; it
   never deletes a row, relaxes one, or edits the work. Ask it to judge, not
   to fix.
5. Hand a failing check to the implementer as the check and its output, never
   as a summary of the failure.

## What this does not buy

The child can write a weak check: one that passes on the broken tree as well
as the fixed one proves nothing. Read a check before you trust it, and prefer
the ones that fail on code you already know is wrong.
