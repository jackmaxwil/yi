# Channel threads and the actor gate

Status: built in this change. Inspired by deepseek-ai/dsh-superagent-prototype
(docs/001-domain.md, docs/007-architecture.md): its Thread and Mandate concepts, cut to the two
that Yi's channels lacked once a design critique removed the rest.

## What shipped

1. **Threads.** A message whose `data.thread` names a conversation joins the open todo of that
   thread as follow-up input; its id lands in that todo's stamp, so a redelivery wakes nobody
   twice. A message on a new thread opens its own todo whatever the job's overlap. Messages with
   no thread keep the old rule (one todo per batch, held by overlap).
2. **The actor gate.** A filter term `key=a|b` matches any listed value, so
   `actor=alice|bob` admits only those people before any model reads the message.
3. **The Forgejo PR adapter** names each head's PR as its `thread` and its author as its `actor`.

## What the critique cut, and why

- Plugin rows that ship adapters: dropping an executable in `~/.yi/adapters/` already works, and
  the example adapters have no pinned release to point a row at.
- A stored mandate: the gate is the filter, re-run on every message, so a stamp would duplicate it.
- Bot lists and logged rejections: nothing would read them yet.
- A write side with an outbox: forge comment APIs cannot refuse a duplicate, so "exactly once"
  needs adapter-side state; and without a self-echo filter every comment Yi posts would wake it.
  That is its own issue, after threads have run on real PR traffic.
- Third-party plugin manifests: the curated table stays the only registry.

## Ceiling

A thread's open todo is found by scanning the list; fine at today's list sizes.
