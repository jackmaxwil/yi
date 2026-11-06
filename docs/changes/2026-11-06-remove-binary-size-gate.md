---
issue: Closes #2050
decision: the dist binary size gate is removed, reversing D249/D424's measured ratchet and hard cap | the measured Linux ratchet failed merge-queue candidates whenever a batch grew the binary past 16 KiB jitter (#1891 alone: 14,334,344 against 14,290,184, three candidates in a row) and every growing PR needed its own baseline-only commit | restore scripts/guardrails/check_binary_size.py and its baselines from git history
---
The dist binary size gate is gone, reversing D249/D424's measured ratchet and hard cap. The
Linux ratchet failed merge-queue candidates whenever a batch grew the binary past 16 KiB jitter;
#1891 alone measured 14,334,344 bytes against 14,290,184 on three candidates in a row, and every
growing PR needed its own baseline-only commit. The guardrail, baselines, CI measurement and
automatic ratchets are removed. The startup budget stays, with local full guardrails still
building the dist binary for it; CI skips that build because it never times startup. Historical
measurements remain in the size ledger.
