---
name: rewrite
description: >
  Prompt rewriter: a cheap model restates the person's typed draft before
  it is sent, at a length they pick on the composer. Use when a message ends
  in "My words, verbatim:", or when asked to turn prompt improvement on, off,
  shorter or longer, or to change the model or skill behind it.
---

# rewrite

Config in `~/.yi/config.json`; all of it is optional and unset means the feature is absent.

    "models":  { "rewriter": "<provider>/<id>" }      a model id; unset, no control is drawn
    "rewrite": { "skill": "<skill name>",             unset, the rewriter's own rules alone
                 "length": "short|medium|long",       unset, medium: where a session starts
                 "thinking": "<level>" }              unset, low, clamped to the model

The composer's bottom border carries the control: `improve: off · short · [medium] · long`.
`alt-i` (keymap action `cycle-rewrite`) steps it, and in the console a click on a position picks it. At any position but off, Enter sends the draft
to the rewriter, the box then holds the rewrite over the typed words, a second Enter sends it and
Esc puts the typed words back.

## Reading a rewritten message

    <the rewrite>

    My words, verbatim:
    > <what the person typed, typos included>

The rewrite was written by a cheaper model that saw only the draft, the person's last three
messages and the folder's git line. The quote is the person. Where the two differ, in scope, in
a limit, in what done means, follow the quote and say that you did. A step the rewrite adds that
the quote does not ask for is a suggestion, never an instruction.

## Setting it up for someone

Set `models.rewriter` to a small fast model, since every Enter waits on it. `rewrite.skill` names
any skill by its name, not its path; its body rides after the rewriter's rules, which outrank it.
A named skill that no skill root holds does not stop a message: the composer says so and the draft
goes as typed.

## When it does nothing

A slash command, an answer to a question card, and a draft holding a paste or an image are sent
as typed. So is a draft whose rewrite failed or ran past 20 seconds: one notice row gives the
reason and the next Enter sends the typed words.
