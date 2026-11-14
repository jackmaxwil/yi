---
cut: none .forgejo/workflows/review.yml
cut: none .forgejo/workflows/autofix.yml
---
The review bot and the autofixer's low tier move from DeepSeek V4.1 Flash to Claude Haiku 5.5 (`openrouter/anthropic/claude-haiku-5.5`) at `xhigh` thinking. Artificial Analysis's free tier scores it 42.5 on its engineering index at $0.20 per million blended tokens, against 34.4 to 37.6 at $0.525 for DeepSeek. OpenRouter lists `xhigh` as a supported effort for it, so the level is not clamped. The medium and high tiers are unchanged.

Both workflows drop the `wafer/mxfp4` routing pin written to `~/.yi/config.json`. It was DeepSeek-specific (a host measured fast for that model) and Haiku is not served there, so a strict pin would refuse every round. Revert by restoring the pin and the two model ids if Haiku rounds run slow.
