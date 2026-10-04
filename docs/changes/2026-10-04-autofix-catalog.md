---
issue: Refs #976
---
The autofix job refreshes Yi's OpenRouter catalog before it runs (Refs #976). A fresh runner knew
only the bundled catalog, which lacks Opus 5.5, so every high-tier fix failed with "unknown model".
