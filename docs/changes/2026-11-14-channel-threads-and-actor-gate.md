---
issue: Closes #2230
cut: none crates/runtime/src/schedule/channel.rs
shares: #2232 both edit docs/YI_DESIGN.md, this one only the channel job paragraph of the schedule section and #2232 only the intro and the provider section
ledger+: channels: buffer, subscriptions, adapters | a message whose `data.thread` has an open todo joins that todo as follow-up input and a new thread opens its own whatever the overlap; a filter term `key=a|b` admits any listed value, so `actor=alice|bob` gates who can start work before any model reads it; the Forgejo PR adapter names each head's `thread` and `actor`
raise: crate runtime +86, tests +135, comments +1
---
A channel subscription now keeps one todo per conversation. A message whose data names a `thread` (a PR, an issue) joins the open todo of that thread as follow-up input, so a reviewer's reply or a new push reaches the work already under way and no longer waits for it to close. A message on a new thread opens its own todo, even while another thread's todo is open. Messages that name no thread behave as before.

A filter term can now list alternatives: `actor=alice|bob` admits messages from either person and nothing else, checked on the host before any model sees the message. The Forgejo PR adapter tags each head with `thread: pr-<n>` and `actor: <author login>`, so the PR-review subscription can use both.

Both ideas come from the Thread and Mandate concepts in DeepSeek's dsh-superagent prototype (docs/001-domain.md, docs/007-architecture.md), cut to the two that Yi's channels lacked once a design critique removed the rest. Cut from that critique:
- Plugin rows that ship adapters: dropping an executable in `~/.yi/adapters/` already works, and the example adapters have no pinned release to point a row at.
- A stored mandate: the gate is the filter, re-run on every message, so a stamp would duplicate it.
- Bot lists and logged rejections: nothing would read them yet.
- A write side with an outbox: forge comment APIs cannot refuse a duplicate, so "exactly once" needs adapter-side state; and without a self-echo filter every comment Yi posts would wake it. That is its own issue, after threads have run on real PR traffic.
- Third-party plugin manifests: the curated table stays the only registry.

Ceiling: a thread's open todo is found by scanning the list; fine at today's list sizes.
