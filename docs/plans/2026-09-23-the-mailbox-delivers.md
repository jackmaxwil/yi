# The mailbox delivers: M1-M3

```
status:  planned 2026-09-23. Lands as docs/plans/2026-09-23-the-mailbox-delivers.md in
         M1. Stages M1-M3 land as one stacked PR on #483 (branch claude/yi-os-mailbox),
         one changelog row, one D-row ADR and one forge issue (milestone 11) per stage.
tree:    0.292.0, last decision row D229 (the #483 tip, 9775f0a1).
```

## Context

A 2026-09-23 audit of the family mailbox at 0.292.0 read the code, mined 291 recorded
sessions (70 parents, 221 children, all `z-ai/glm-5.3-flash`, from the g2, final and
confirm3 confirmations) and ran eight real `yi ask` trials for $0.097. It found that a
message's fate depended on which of several roads its sender happened to take:

- **Order.** A plain `inform` rode a turn-end queue and everything else the steer queue,
  so presentation order was not send order. The service trial got a total of 12 instead
  of 22; the busy-child trial presented CHERRY before BANANA and the child answered twice.
- **Stranding.** Three wake primitives with three guarantees (`heartbeat_hook`,
  `wake_idle_hook`, `notice_hook`), chosen per call site. The run loop's early stops and
  the flip to idle went idle without looking at either queue, so a message queued in
  that window waited for a turn nobody started.
- **Woken turns were invisible.** A worker's exit stayed `Completed` after its first
  run: status read `finished` all through a woken turn, `rlm.wait(60)` slept 60 s on a
  child that answered in 3 s, and no notice came for the second answer.
- **Redundant wakes.** All 23 finished and stuck notices in the corpus arrived after the
  parent's final answer and each woke it again; 7 of 8 trials ended on a "nothing new"
  turn; 176 of 187 reaps came doubled with a plan verdict line.
- **The breaker parsed Python.** Its wait exemption matched cell text, so a waiting cell
  that printed `await rlm.status()` was steered as a repeat.
- **Also:** asking the parent through `ask_user` was eaten by the todo intercept (U1),
  nothing could wait for mail (FN1), protocol checks ran in the process cwd (FN3), a
  headless run abandoned the model's own children (G1), `rlm` was async-only and had
  drifted from the module (U2, U3), and the human could not see or answer a question (V1).

The owner approved all ten recommendations. Each removes a decision or a silent failure
mode rather than adding refusal text:

1. One ordered delivery queue per recipient, inbox first, with a durable presented cursor.
2. Every turn of a child is a run.
3. Asking is a request, not an ending.
4. A receive primitive for every member.
5. A headless run owns its children's lifetime.
6. A notice the model has already acted on does not wake it.
7. Checks name where they run.
8. One Python surface that works without `await`.
9. The human sees the model's facts.
10. `wait` explains itself, and the breaker asks the host.

## M1. The queue delivers in order and nothing is stranded (items 1, 2, 6, 10)

- One queue per session: mail, host notices and steers enter it in arrival order, each
  entry carrying whether it wakes an idle session and, for a lifecycle notice, a test of
  whether it is still news. A run presents it at every message boundary.
- Every way a run ends (an early stop, an error, an abort, the loop's own end) goes
  through one settle step that, under the status lock every enqueue takes, starts the
  next turn if a waking entry or a follow-up is owed, else goes idle. `followup` only
  wakes an idle receiver; agent mail has no turn-end mode.
- The presented copy is the transcript's own message entry, so an inboxed envelope with
  no presented copy is queued again when a store is attached after a crash.
- A run that starts on a concluded child record is another run: it reads running, moves
  the epoch, and one reader concludes it. Each run bills only its own turns.
- The host records the epoch of each ending the owner read through `result`, `wait` or
  `status`; a notice for an ending already read is dropped unread. A finish that reaps
  its child says its verdict and the reaped product as one line.
- `wait` names why each child moved (`causes`), every envelope up moves the epoch, and
  the repeat breaker reads the host's count of family waits instead of the cell's text.

## M2. Asking, receiving, headless ownership, checks (items 3, 4, 5, 7)

- In a child session `ask_user` routes as `request("parent")`: the cell blocks, the
  parent sees `needs_you` with a reply id, and `reply_to` is the one answer call. The
  `ask_user` exemption moves ahead of the empty-stop intercept.
- `await rlm.receive(timeout, from_=None, kind=None)` over the inbox and M1's cursor.
- `yi ask` holds while any owner child is live or any waking mail is queued, bounded by
  `--deadline`, and names every live child and undelivered envelope at exit.
- The cwd-less `goal::run_check` goes; the check runner takes the child's worktree or
  the session cwd from its record.

## M3. One Python surface, and the human's view (items 8, 9): done

Item 8 landed as 0.294.0 (D232) and item 9 as 0.296.0 (D234, #498), with the M2 review's
seven fixes. The human's answer and the parent's resolve the same request: the first wins
and the other is refused by name.

- Item 8, in parallel with M1: the `rlm` object's methods generated from the module's
  functions, and a sync form of every call when it is not awaited.
- Item 9, after M2: cards show `MemberState` and the question, a reply box sends a
  `reply` envelope, replay keeps `agent_message`, and ACP marks host notices as host.

## Verification

Each stage lands with tests that go red without it, reproducing the audit's trial where
one exists. After M3, the audit's eight dogfood trials (`mbx-ask`, `mbx-request`,
`mbx-steer`, `mbx-siblings`, `mbx-service`, `mbx-fanout`, `mbx-discovery`,
`mbx-detach`) are rerun on `z-ai/glm-5.3-flash` through `evals/surface.py`: the
service total is 22, BANANA and CHERRY arrive before the answer, the woken child's
second answer is waited on and noticed, no trial ends on a "nothing new" turn, the
protocol child's check passes, and the detached child's `done.txt` exists.
