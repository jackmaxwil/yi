---
name: models
description: >
  Suggest a model per role (primary, advisor, summarizer, autoReview,
  selector) from OpenRouter's Artificial Analysis benchmarks and
  the catalog's prices. Use when the user asks which model to use, says a
  model is too slow, too costly or not smart enough, or asks to compare
  models. Suggestions only: never change config without the user's word.
trigger: which model, recommend a model, cheaper model, model for the advisor
---

# models

`yi models recommend [--role R]` → read the three picks → offer one → the user sets it

## What it prints

The citation line and the snapshot's `as_of` come first, then how long ago the snapshot was
fetched, then one bracket row counting the OpenRouter catalog models it scored and the ones it
left out (no intelligence index, no OpenRouter credential, no tool calling). Each role prints its
formula, its floors, the token mix it priced at, and three rows:

- `high`: the most intelligent model above the role's floors.
- `medium`: the best `intelligence^exponent / blended $ per Mtok x popularity`.
- `low`: the cheapest model above the floors.

One model can hold two tags; that means it is both the cheapest and the best ratio.

## Reading the arithmetic

- The blended price weights input, cache-read, cache-write and output by a token mix. The
  primary and advisor roles use the mix from the user's last 7 days of root sessions, or
  the default 97.8% cache-hit mix when there are none, so cache-read price usually decides. The
  summarizer, autoReview and selector roles read fresh text each call and use their own
  mostly-uncached mix.
- The advisor squares intelligence: one advice per review costs little, so a smarter model pays
  for itself. The summarizer needs a 200k context and an intelligence floor of 30.
- Popularity is a multiplier of at most 1.03 for the daily top model by OpenRouter tokens, so a
  ratio more than 3% better always wins.

## When the run fails

- `no benchmark snapshot ... run yi models refresh`: offer to run `yi models refresh`. It needs
  `OPENROUTER_API_KEY` or `yi login openrouter`, and it spends 3 of the key's 30 requests a minute.
- `HTTP 429`: the key hit OpenRouter's limit; the old snapshot stays readable, so recommend from it
  and name its age.
- `refresh waits 600s between fetches`: the snapshot is fresh, so use it.
- `0 of N scored ... without a credential`: the user has no OpenRouter key, so nothing is routable.

## Applying a pick

Name the pick and its arithmetic, then let the user decide. When they agree, the role is a key in
`~/.yi/config.json` under `models` (`models.advisor`, `models.summarizer`, ...), and its value is
the row's id as printed (`openrouter/<id>`).
