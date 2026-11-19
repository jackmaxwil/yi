---
issue: Closes #2191
cut: none crates/ai/src/request.rs
growth: +17 one shared check, one deleted override, two tests
decision: a reported zero is an unknown turn when it counts no tokens and no charge | a relay's zero counts are not a measured free turn, so the session cost header must not read them as complete | revert by clearing usage.unknown in openai.rs and openai_responses.rs
raise: crate ai +17, tests +58, comments +2
---
Turns stored with zero usage came from two places: the Cursor provider, whose stream carries no token counts yet overwrote the unknown default with a measured zero on every turn, and the OpenAI-compatible mappers, which stored a usage object counting nothing (`{}`, or zero tokens and zero charge) as a free turn. Both now store the turn as unknown, so the session cost header reads as a lower bound instead of complete. The faux provider's zeros are test fixtures, not a bug.
