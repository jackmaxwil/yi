# Channel adapters

An adapter feeds a Yi channel (D287). It is any executable named `yi-adapter-<scheme>`,
in any language, found in `~/.yi/adapters/` first and then on `PATH`. A subscription to
`<scheme>://…` (`rlm.subscribe`, or a todo blocked `on` the address) starts it.

## Protocol

Yi runs `yi-adapter-<scheme> '<the whole URI>'` in the subscription's working directory.

- **stdout, one JSON object per line:** `{"id": "<unique per message>", "at": <epoch ms>, "data": {…}}`.
  `id` is the idempotency key: an id the channel already holds is not appended again.
  `at` defaults to the time Yi read the line. `data` serialized is at most 16 KiB; a bigger
  one is kept as a refusal naming the cap, so send a `store://` or path reference instead.
- **stdin, one JSON object per line:** `{"ack": "<id>"}` once the message is synced to the
  channel's buffer, with `"refused": "<why>"` added when Yi kept a refusal in its place.
  Drop your copy (delete it from the queue) only on an ack; without one, send it again.
- **stdin closing** means Yi is gone: exit.
- **stderr** goes to `<channel>.log` beside the buffer.
- **Exiting** is a crash. Yi restarts the adapter up to 3 times in 10 minutes; past that it
  stops, tells every subscribing session why, and pauses their subscriptions until one is
  resumed (`rlm_heartbeat.update {"id": …, "status": "resume"}`). `/heartbeat halt` stops
  every adapter until `/heartbeat resume`.
- A URI ending `?every=<n>s|m|h` sets the adapter's own poll cadence, by convention.

Yi runs one adapter per channel per process, while a subscription reads it at least once
every three of its cadences.

## Built in

- `exec://<command>?every=30s` runs the command with `sh -c` on the cadence and emits
  `{ok, exit, output}` when its exit status changes (the newest message is the current
  level, so a wait on `ok=true` unblocks when it is green).
- `file://<path>?every=5s` emits `{path, exists, size, mtimeMs}` when any of them changes.

## Examples here

Each is stdlib Python, tested against a fake CLI by `test_adapters.py`:

- `yi-adapter-github` — `github://<owner>/<repo>?every=60s`: each finished workflow run, via `gh api`.
- `yi-adapter-forgejo` — `forgejo://<host>/<owner>/<repo>?every=60s`: each run whose jobs all
  ended, with its conclusion, via `fgj api`.
- `yi-adapter-sqs` — `sqs://<queue url without https://>`: `aws sqs receive-message`
  long-polls, and a message is deleted only after Yi acks it.

Install one by copying it into `~/.yi/adapters/`.
