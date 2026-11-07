---
issue: Closes #2054
raise: comments +10, tests +31, crate cli +185
growth: +185 first-run setup became menus with an inline key step and word answers; weighed deleting the other-login rows and the word matching, kept both because a kimi or codex login was invisible and a typed provider name was rejected, and deleted the whole classifier step and the free-text provider and model prompts
decision: `yi setup` asks two numbered menus, takes the key of the provider picked, and always records that it ran, amending D300 | the first-run flow asked free text, ended with no config when nothing was chosen so every launch asked again, saved a model whose key it never took, and a dogfood as a first-time user found the jargon and dead ends; the classifier question is dropped because `models.classifier` is already a config key | restore the free-text prompts, classifier_step and the no-write finish in crates/cli/src/setup.rs
ledger~: first-run setup (`yi setup`) | the model, the saved permission mode and the optional classifier, each skippable, written | the model (with the key pasted for a provider that has none) and the saved permission mode, each a numbered menu whose Enter answer is the safe default and a word that names one row, written
---
`yi setup` is two numbered menus in plain words. The first asks which AI Yi should use: a flagship
for Anthropic, OpenAI, OpenRouter and Google, plus a model for any other provider a stored login
already unlocks, the ones with a credential first and the first of them the Enter answer. A
config that already names a model is offered "Keep" instead. Picking a provider with no key asks
for it right there, with the page that makes one, and stores it as `yi login` does. The second
asks how much Yi should ask before it acts and defaults to the mode already saved. A number or
one word that names a single row both answer; a mistyped answer is asked again and is never read
as a skip. Finishing with nothing chosen writes `{}`, as declining does, so the first-launch offer
does not return on every launch. The classifier question is gone: `models.classifier` stays a
config key, and `yi doctor` says so.

`yi login anthropic` now takes a pasted key like openai, openrouter and google. It refused with
"no login profile for anthropic" while the missing-key message told people to run it. A login
whose models arrive only with the account's list (kimi, cursor) appears in the menu once that
list has been fetched.
