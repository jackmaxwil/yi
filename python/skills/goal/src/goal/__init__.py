"""Yi goal skill: session goal state from the kernel.

The goal lives host-side as a session-store fact (it survives compaction by
construction); these functions are thin typed wrappers over the generic host
bridge (`rlm.host_request`). They only work inside the Yi IPython kernel.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def get() -> dict[str, Any]:
    """Read the current goal: objective, status, budgets, usage, remaining."""
    return await host_request("goal.get")


async def create(objective: str, token_budget: int | None = None) -> dict[str, Any]:
    """Create a goal only when explicitly requested; never infer one.

    Fails while an unfinished goal exists — use `update` to finish it first.
    Set `token_budget` only when an explicit token budget was requested.
    """
    if not isinstance(objective, str) or not objective.strip():
        raise ValueError("objective must be a non-empty string")
    payload: dict[str, Any] = {"objective": objective}
    if token_budget is not None:
        if not isinstance(token_budget, int) or token_budget <= 0:
            raise ValueError("token_budget must be a positive int")
        payload["token_budget"] = token_budget
    return await host_request("goal.create", payload)


async def update(status: str) -> dict[str, Any]:
    """Report terminal state: "complete" or "blocked" only.

    Mark "complete" only when the objective is achieved and verified.
    Mark "blocked" only after the same blocker has recurred for at least
    three consecutive goal turns. The host owns pause/resume/limits.
    """
    if status not in ("complete", "blocked"):
        raise ValueError('status must be "complete" or "blocked"')
    return await host_request("goal.update", {"status": status})
