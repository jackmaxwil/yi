---
name: baseten
description: >
  Run a model deployed on Baseten as a Yi provider through a `providers.baseten` entry in
  `~/.yi/config.json`. Use when the user wants Baseten, gives a `model-<id>.api.baseten.co` URL,
  or a baseten/ model is refused, 401s or 404s.
---

# baseten

Yi ships no Baseten row: a deployment is a config provider on the `openai-completions` wire.
`baseten/<served-name>` then works like any model.

## The entry

Add to `~/.yi/config.json` (read once per process; restart after an edit):

```json
{"providers": {"baseten": {
  "baseUrl": "https://model-<MODEL_ID>.api.baseten.co/environments/production/sync/v1",
  "apiKeyEnv": "BASETEN_API_KEY",
  "models": [{"id": "<served-name>", "name": "<label>", "contextWindow": 131072, "maxTokens": 8192}]
}}}
```

- `<MODEL_ID>` is in the model page's URL in the Baseten workspace. Another environment is
  `/environments/<name>/sync/v1`; one deployment is `/deployment/<id>/sync/v1`.
- `<served-name>` is the name the deployed server accepts, not the Baseten model name. For vLLM it
  is `--served-model-name` in `start_command`. A wrong one is a server 4xx naming the model.
- `api` is unset, which means `openai-completions`. Baseten takes the key as `Authorization:
  Bearer`, which that wire sends. Never put the key in `config.json`: export `BASETEN_API_KEY`.
- One base URL is one deployment. A second deployment is a second provider id
  (`baseten-small`), since the model id rides the host.

## Check it

1. `yi doctor`: the `providers` row names `baseten` and where its key resolved.
2. `yi ask --model baseten/<served-name> "Reply with exactly: Paris."` reaches the deployment.

## When it fails

| Symptom | Cause and fix |
|---|---|
| refusal naming `BASETEN_API_KEY` | the variable is unset in Yi's environment: export it, restart |
| 401 | key lacks access to that deployment, or is for another workspace |
| 404 | base URL stops short of `/v1` or names a wrong model id; Yi appends `/chat/completions` |
| 400 on tools or schema | the deployed server cannot enforce them; Yi sends no strict tools on a config provider |
| timeout with no output | a scaled-to-zero deployment is cold; the 60 s read bound is fixed for config providers, so retry once warm |

A custom Truss model (`/predict`) or custom HTTP server that is not OpenAI-compatible has no chat
wire: Yi cannot call it as a provider.
