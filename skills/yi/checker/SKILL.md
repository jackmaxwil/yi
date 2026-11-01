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
2. Spawn one worker walled from history and from the workspace, with the task
   text in its brief and nothing of this transcript. Bind three variables
   first. `task_text` is the task text as one JSON string literal (quotes,
   backslashes and newlines escaped): the text is data, never source, so a
   `"""` in it cannot close the literal and run as Python in your kernel.
   `workspace` is the workspace root, the environment block's `cwd`, also a
   JSON string; never `os.getcwd()`, which any earlier `%cd` or `os.chdir`
   moved. `checks_dir` is a fresh directory outside the workspace, the one
   place the child writes:

```python
import tempfile
task_text = <the task text as a JSON string>
workspace = <the workspace root as a JSON string>
checks_dir = tempfile.mkdtemp(prefix="checker-")
worker = await rlm.run(
    f"The task text:\n\n{task_text}\n\nFrom that text alone, write "
    f"{checks_dir}/checks.py: one `PASS <id>` or `FAIL <id>` line per "
    "requirement, runnable as python3 with no arguments from the workspace. "
    "You cannot read the workspace; do not write a fix; a check that cannot "
    "run says why.",
    role="worker",
    deny_url=["history://"],
    deny_read=[workspace],
    deny_write=[workspace],
)
print(worker.name, checks_dir)
```

The three walls are the point, not a preference. `deny_url=["history://"]`
keeps every transcript (yours, this session's parent's, a sibling's) out of
its view; `deny_read` and `deny_write` on the workspace root keep it from
opening the attempt or editing the tree it is grading. Its checks come from
the task text instead of from a copy of what you did, and `checks.py` lands in
`checks_dir`, never in the graded tree.

The child's one write lands outside the working tree, so in Auto mode it is a
permission ask: each run needs a person to approve it, or an allow rule
covering the temp directory. A headless run (`yi ask`, an unattended run, a
plan child) has no one to ask, refuses that write, and leaves you grading
yourself again; there, run with `--yolo` or that allow rule, or do not use
this recipe.

3. Start every requirement row with the check that proves it, and step the row
   done only on that check's own exit or `PASS` line. In a plan, the row's
   `check=` is its contract's `decider: {cmd}` and the engine runs it; with no
   plan, run the checks program yourself before each row's `done`.
4. You run the checks on the finished work: `python3 <checks_dir>/checks.py`
   from the workspace. A reader cannot run anything (its tools are `read` and
   `grep`) and writes nothing, so hand a reader the task text and that output
   and ask only which requirements the checks miss. You append those rows; no
   one deletes a row, relaxes one, or edits the checks to make them pass.
5. Hand a failing check to the implementer as the check and its output, never
   as a summary of the failure.

## What this does not buy

The child can write a weak check: one that passes on the broken tree as well
as the fixed one proves nothing. Read a check before you trust it, and prefer
the ones that fail on code you already know is wrong.
