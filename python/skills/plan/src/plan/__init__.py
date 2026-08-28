"""Yi plan skill: the session task DAG over the host bridge.

These calls proxy to host requests through the kernel comm channel
(`rlm.host_request`). They only work inside the Yi IPython kernel.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def get() -> dict[str, Any]:
    """Read the plan: tasks, states, frontier (ready task ids), finished."""
    return await host_request("plan.get")


async def create(tasks: list[dict[str, Any]]) -> dict[str, Any]:
    """Create the session plan from task specs.

    Each task: {title, acceptance, id?, deps?, check?, schema?, assignee?}.
    `acceptance` is what must be true when the task is done; `check` is a
    shell command the host runs to verify a done claim (exit 0 = pass).
    Fails while an unfinished plan exists — grow it with edit_add instead.
    """
    if not isinstance(tasks, list) or not tasks:
        raise ValueError("tasks must be a non-empty list of task dicts")
    return await host_request("plan.create", {"tasks": tasks})


async def update(
    task_id: str,
    state: str,
    evidence: dict[str, Any] | list[Any] | str | int | float | bool | None = None,
    reason: str | None = None,
) -> dict[str, Any]:
    """Report a task state: running | done | blocked | pending.

    A done claim is verified by the host (check + schema); on failure the
    task becomes blocked with the evidence. Blocking requires a reason.
    """
    if state not in ("running", "done", "blocked", "pending"):
        raise ValueError('state must be "running", "done", "blocked" or "pending"')
    payload: dict[str, Any] = {"task_id": task_id, "state": state}
    if evidence is not None:
        payload["evidence"] = evidence
    if reason is not None:
        payload["reason"] = reason
    return await host_request("plan.update", payload)


async def edit_add(tasks: list[dict[str, Any]]) -> dict[str, Any]:
    """Add tasks to the plan (expand-only; removal needs the user)."""
    if not isinstance(tasks, list) or not tasks:
        raise ValueError("tasks must be a non-empty list of task dicts")
    return await host_request("plan.edit", {"action": "add", "tasks": tasks})


async def edit_reopen(task_id: str) -> dict[str, Any]:
    """Reopen a done task to pending."""
    return await host_request("plan.edit", {"action": "reopen", "task_id": task_id})
