---
name: unattended
description: >
  Finish a task alone when hidden checks will grade it: turn the task text
  into requirements, give each a check, and end on one cell that prints PASS
  or FAIL per requirement. Use when no person is attached to the run, when a
  task says it will be tested or graded, or before reporting a task done.
  Do NOT use to pick what to build; the task text decides that.
---

# Finishing alone

A run that nobody watches is judged once, at the end, by checks written from the task's text.
Runs like this rarely fail to start. They fail on the last few checks: a file one directory off,
a header with a trailing space, an error path that exits 0, a case the examples never showed.
Every point is in the text, so the work is to read the text as the grader will.

## The requirement list

Read the task twice and write each requirement as a todo, in the text's own words:

| Look for | Its check |
|---|---|
| A path, file name or directory | the exact path exists, and nothing was written elsewhere |
| An output format: columns, header, order, delimiter, JSON keys, trailing newline | compare bytes with a sample built by hand from the text |
| A command, endpoint or function that must work | run it the way the text shows, from a fresh shell |
| An error case, a bad input, an exit code | feed the bad input and read the exit code and message |
| A limit: time, memory, size, count | measure it on the largest input you can make |
| Something that must keep running, such as a service or a port | it answers after your shell exits |
| Something that must not change: given tests, fixtures, other files | `git status` or a checksum before and after |
| An implied case: empty input, one item, Unicode, duplicates, a rerun | run each one; graders add the cases examples skip |

Run any tests the task ships first: they show the grader's harness and naming.

## The verifier cell

Keep the checks in a kernel variable, so each fix is followed by the same full run:

```python
import pathlib, subprocess

def sh(command, timeout=120):
    done = subprocess.run(["bash", "-lc", command], capture_output=True, text=True, timeout=timeout)
    return done.returncode, done.stdout, done.stderr

checks = {
    "R1 /app/report.csv exists": lambda: pathlib.Path("/app/report.csv").is_file(),
    "R2 header is exactly owner,total": lambda: pathlib.Path("/app/report.csv").read_text().splitlines()[0] == "owner,total",
    "R3 bad input exits non-zero": lambda: sh("python3 /app/tool.py /nonexistent")[0] != 0,
}

def verify():
    failed = 0
    for name, check in checks.items():
        try:
            ok = bool(check())
        except Exception as error:
            ok, name = False, f"{name} ({type(error).__name__}: {error})"
        failed += not ok
        print("PASS" if ok else "FAIL", name)
    print(f"{len(checks) - failed}/{len(checks)} requirements hold")

verify()
```

Add a check when you find a requirement, never remove one to make the count look better, and
call `verify()` again after every fix. Run commands through `bash -lc` from the directory the text
names, so they see what a fresh grader shell sees, not your session's variables.

## Time

The run has a deadline. Spend the first part reading and listing, and keep the last tenth for a
clean rebuild, the full `verify()`, and the report. A failing check late in the run gets a small,
targeted fix, never a rewrite. Finishing early with time left means a case went untested: make
another input and try it.

## The report

List each requirement with its PASS or FAIL line from the last `verify()`, every assumption you
made where the text was silent, and anything you could not check and why.
