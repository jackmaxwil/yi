---
issue: Closes #1195
raise: tests +45, comments +4, crate runtime +40
---
A todo call that carries only `evidence` is now shown the done call that lands (Closes #1195). The `op is required` refusal names the one running item by id and carries the model's own evidence, `{"op": "done", "id": "t2", "evidence": "<the evidence it sent>"}`, in place of the fixed t1 example with someone else's command. When no item or more than one runs, the old example stays. The op is still not inferred, so a call that names no item never closes anything: inference stays limited to the field sets D155 lists. A repeated refusal from any tool no longer ends in a `next: call <tool> as …` line when that template fills no missing required key, since such a template is the refused call itself. Dogfooding on openrouter/z-ai/glm-5.3-flash on 2026-10-10 recorded ten identical evidence-only refusals in one session: the model was shown its failing call as the next step and sent it again.
