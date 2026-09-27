# evals/trials — the trial store

One file per paid run, `<run-id>.jsonl`, one JSON row per harbor trial as
`drivers/trials.py rows` merges it: `task`, `trial`, `reward`, `partialScore`,
`censored`, `errored`, `timedOut`, `wallSec`, the token columns, `costUsd`
(`null` when a turn was unpriced), plus `at` (epoch seconds) and `arm` (`defaults`,
`levers:<sha12>`, or the caller's `EVAL_ARM`).

Rows carry task ids and numbers, never task content: Terminal-Bench files never
enter git (the public mirror carries this directory). The session files the rows
were scored from stay outside the repository, under `~/Development/yi-runs/<run-id>/`.

The store is also the budget's ledger. `drivers/trials.py caps` sums this ISO
week's rows and the run's own rows, charging an unpriced trial $1 (the watcher's
per-trial cap), and refuses a call whose predicted cost (tasks x $0.13) would pass
the stage's $8 or the week's $25 soft cap. Its hard cap is the smaller of what the
stage's $10 and the week's $30 leave, and `watch.py` stops the stream there.
