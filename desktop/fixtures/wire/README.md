# Wire recordings

Every frame between a client and a real `yi serve`, both directions, one JSON object per line: `{"dir": "send" | "recv", "frame": <the JSON-RPC frame>}`. Written by `spikes/wire/run.sh` (the `spike-wire` scenes), never by hand.

- Produced by `yi 0.1041.0` (`yi --version`), model `faux/faux-1` (the two `child-*` recordings excepted, see their rows), server started `yi serve --socket <home>/yi.sock --model faux/faux-1 --confirm` in a throwaway `HOME` under `target/wire/home`.
- Paths replaced: the server's home is `$HOME`, the scratch project served by the cassette worker is `$PROJECT`, the plain faux project is `$OTHER`, the lane root is `$LANE`. Session ids, entry ids, ask ids and timestamps are left as recorded.
- `yi serve` refuses `--faux`, so the approval and resume scenes first swap the `$PROJECT` worker with `_yi/worker_replace` for `spikes/wire/faux-worker.sh` (the real `yi acp` fed `spikes/wire/cassette.jsonl`). That swap is not in the recordings.

| File | What happened |
|---|---|
| `empty-list.jsonl` | `initialize`, then `session/list` on a fresh server that has no thread: `sessions` is empty |
| `turn.jsonl` | `initialize`, `session/new`, `session/prompt`, the stream to `state_update idle` |
| `approval-allow.jsonl` | a bash call asks, answered `allow_once`; the command ran |
| `approval-reject.jsonl` | the same ask answered `reject_once`; the command did not run |
| `resume.jsonl` | three connections: the first closes while the ask waits; the second resumes and answers the kept ask id; the third resumes from 0, then from `replayedTo` |
| `stored-list.jsonl` | a thread made by `yi ask` in `$PROJECT` with no server running, then a server: `session/list` without a cwd is empty, with `$PROJECT`'s cwd it holds the thread. Written by `spikes/wire/stored_list.py` |
| `child-turn.jsonl` | Written by `spikes/wire/child_turn.py stop` against the dogfood harness's real model (`openrelay/bedrock-claude-sonnet-5-5`, `--confirm`, every ask answered `allow_once`): a turn whose ipython call starts one child (`sleeper`, `sub-cbcfaa6f`) with `rlm.run`; its events tagged `childId`; `_yi/child_replay` for it (a `_yi/replay` with `childId`, then `{}`), for `no-such-child` (refused `unknown child`), then `_yi/child_abort` (the child ends `interrupted`). The session folder's slug is `--PROJECT-` |
| `child-ask.jsonl` | `child_turn.py ask`, the same server: a `root` child (`asker`, `sub-655c3f0b`) asks its parent, raising its own approval on the way; its `_yi/subagent_update` carries `needs_you` with the note `asks asker-1: …`; `_yi/child_answer` answers it; the child finishes `completed`, and `_yi/child_replay` replays its whole conversation |
| `tools-turn.jsonl` | Written by `spikes/wire/tools_turn.py SOCK $PROJECT OUT --rig spikes/wire/tools-worker.sh` against `yi serve --model faux/faux-1 --confirm` in a throwaway home (the dogfood relay did not answer): the cassette worker (`tools-cassette.jsonl`) asks for bash with ANSI colour, read `src/lib.rs`, grep `fn mean`, two ipython cells (JSON, then a `ZeroDivisionError` traceback), read and edit `README.md`, and `git diff`; Yi ran every tool on a copy of the dogfood `tally` seed, every ask answered `allow_once` |
| `lists.jsonl` | `session/list`, `_yi/pin`, `_yi/seen`, `_yi/fork` at and before, `_yi/settings`, `_yi/set_default`, `session/set_config_option`, `_yi/branch_diff`, and four refused calls |
| `kernel-cells.jsonl` | Written by `spikes/wire/child_turn.py cells` against the dogfood harness's server (its model relay did not answer, so the `$PROJECT` worker was first swapped with `_yi/worker_replace` for `spikes/wire/kernel-worker.sh`, the real `yi acp` on `faux/faux-1` fed `kernel-cassette.jsonl`): five `ipython` cells the real kernel ran, every ask answered `allow_once`: stdout and stderr, a Markdown value (`resultMime` `text/markdown`), a pandas DataFrame (its `text/plain`), a Pillow chart shown through the kernel's attachment display (an `image/png` block; faux-1 has no vision, so `attach_image` itself refuses), and `ValueError('boom')` with its traceback |

Re-record: `spikes/wire/run.sh` (about 30 s). Decode check: `cargo test -p spike-wire`.
