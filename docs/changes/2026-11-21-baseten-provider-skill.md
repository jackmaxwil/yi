---
issue: Closes #2336
cut: none skills/yi/baseten/SKILL.md
growth: +0 skill and docs only, no src change
---
Yi now teaches how to run a model deployed on Baseten. A deployment is OpenAI-compatible, so it needs no provider code: the new `baseten` skill gives the `providers.baseten` entry in `~/.yi/config.json` (base URL from the deployment's model ID, `BASETEN_API_KEY` as the bearer, the served model name as the model id), how to check it with `yi doctor` and `yi ask`, and what each failure means. Custom Truss and non-OpenAI HTTP servers stay out of scope.
