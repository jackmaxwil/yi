---
name: annotate
description: >
  How to steer what Yi cuts from your own history: when to pin a tool result,
  when to discard one, when to record a finding instead, and how to get a cut
  result back. Use when a long session is piling up reads and command output,
  when you are about to lean on an old result, and when a "[reclaimed: ...]"
  placeholder replaces something you need. Do NOT use to save a fact you will
  need only this turn, and never as housekeeping after every call: Yi cuts on
  its own.
covers: tool:annotate
---

# annotate

`annotate` → let reclaim decide → pin what you will reuse → discard what is spent → finding for facts

## Leave it alone by default

Yi already cuts old, large tool results once dropping them costs less than resending
them. Most sessions never need `annotate`. Reach for it when you know something the
cut cannot: an old result you will lean on again, a bulky one you are finished with, or a
fact that must outlive compaction.

## Which mark

- **pin** a result you will reuse many turns on: a spec you read once, the baseline test
  output you will compare against. A pinned result is never cut, so pin few; each one is
  resent on every request.
- **discard** a big result you are done with (a full build log, a directory listing, a
  file you decided is irrelevant). It waives the age and size floors, but the cut still
  waits until it pays, so the result may stay whole a while.
- **finding** when the fact matters, not the output: a root cause, a path, a decision.
  The note (280 characters) is listed in every compaction view, so it survives when the
  underlying result is long gone. A finding needs no `call`; give one only to tie the
  note to the result it came from.

The last mark on a result wins: a discard lets go of an earlier pin, and a pin of an
earlier discard.

## Naming the result

`call` matches the newest result whose call contains your text: `read src/lib.rs`, a
distinctive command fragment, or a path. A placeholder quotes the argument in backticks;
either spelling matches. When nothing matches, the error lists the newest five calls
(`the newest 5 of N: bash `ls`, read `src/lib.rs`, ...`); copy a fragment from that list.

## What a discard cannot take

A discard on one of these is refused, and no retry changes that:

- `plan`, `todo` and `ask_user` results, which carry live state or the user's words.
- A result with an image.
- The newest read of a file you edited this session. An edit needs that read's line tags,
  so the refusal reads "the newest read of a file this session edited is never cut". Older
  reads of the same file are still cuttable; name one by a fragment unique to it.

## Getting a cut result back

A cut result is replaced by one row:

```
[reclaimed: bash `cargo build` → none of 9990 chars kept; <rule> (cut at request 10); get it: read("history://self/e-17")]
```

The stored result is untouched. Run the `read` the row names and you have it whole; read
only if you still need it, since the answer sits in your view until the next cut. If the
row says "it is not in this session's store", no restore exists: rerun the original call.

## Refusals

A pin or discard on a result that was "already cut at request N" is refused, and the
message says its placeholder names the read that restores it. To keep a restored copy,
pin the restoring read, named by its entry id (`call: "history://self/e-17"`). A finding is never refused for this.
