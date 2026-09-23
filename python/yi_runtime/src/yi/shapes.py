"""Shapes: schedulers in user space, handed to ``plan.run`` (plan sections 8.5 and 8.6).

A shape checks the plan's geometry before its first ``start`` and then only
chooses what to ask the host for next. Admission, the step table and ``done``
stay the host's: a refused start is asked again, a refused retry is final.
"""
from __future__ import annotations

import asyncio
import json
import re
from typing import Any

import rlm

from .plan import Plan, PlanError, Run, Todo, _pruned
from .roles import _inside, verify_quotes

# OTP's restart intensity for fork_join: at most MAX_RESTARTS within RESTART_WINDOW seconds.
MAX_RESTARTS = 3
RESTART_WINDOW = 600.0
SCATTER_MAX_ROUNDS = 3
# What a scatter reader answers; a null answer abstains (the host's schema subset has no union type).
ANSWER = {
    "type": "object",
    "required": ["answer", "quotes"],
    "properties": {"quotes": {"type": "array"}},
}
ROUND = re.compile(r"-r\d+$")


class Geometry(PlanError):
    """A shape refused the plan before any start; ``problems`` names every reason at once.

        try:
            await plan.run(shape=fork_join)
        except Geometry as refused:
            print(refused.problems)
    """

    def __init__(self, shape: str, problems: list[str]) -> None:
        super().__init__(f"{shape} refuses this plan: " + "; ".join(problems))
        self.problems = problems


def _delegation(todo: Todo) -> dict[str, Any]:
    return todo._doc.get("delegation") or {}


def _reads_only(todo: Todo) -> bool:
    wall = (_delegation(todo).get("spec") or {}).get("wall") or {}
    return "." in wall.get("deny_write", [])


async def _schedule(plan: Plan, run: Run, labels: set[str], restart: bool = False) -> None:
    """Start what is ready among ``labels`` in plan order, settle, repeat until nothing is active.

    Invariant: a start refused for admission is asked again after the next settle and nothing
    behind it is tried first, so the order of starts is the host's order.
    """
    restarts: list[float] = []
    spent = False
    while not run.over:
        await plan.refresh()
        held = {todo.label for todo in plan.unresolved}
        for key in [key for key, refusal in run.refusals.items() if refusal.kind == "admission"]:
            del run.refusals[key]
        for todo in [] if spent else plan.ready():
            if todo.label not in labels or todo.key in run.refusals or todo.label in held:
                continue
            refusal = await run.launch(todo)
            if refusal is not None and refusal.kind == "admission":
                break
        if not run.active:
            return
        for todo in await run.settle():
            if not restart or todo._doc["state"] != "failed":
                continue
            now = asyncio.get_running_loop().time()
            restarts = [at for at in restarts if now - at < RESTART_WINDOW]
            # ponytail: the window lives in this kernel; the host's retry cap is the durable bound.
            if len(restarts) >= MAX_RESTARTS:
                spent = True
                continue
            try:
                await todo.retry()
            except PlanError as refusal:
                run.refusals[todo.key] = refusal
                continue
            restarts.append(now)
            run.refusals.pop(todo.key, None)


async def fork_join(plan: Plan, run: Run) -> None:
    """Run isolated todos side by side: worktree writers and readers, joined by ``after`` edges.

    Refused with ``Geometry`` unless two todos can run at once and every one is
    a ``Writer`` in its own worktree or a ``Reader``. Starts follow plan order up
    to the host's admission count. Restart is one_for_one: a failed todo alone
    is retried, at most ``MAX_RESTARTS`` times within ``RESTART_WINDOW`` seconds
    across the shape; past that nothing new starts and what runs is settled.

        run = await plan.run(shape=fork_join, budget="1h")
    """
    mine = [
        todo
        for todo in plan.todos
        if todo._doc["state"] != "abandoned" and (_delegation(todo) or todo.label in plan._inline)
    ]
    ready = {todo.label for todo in plan.ready()}
    forked = [todo for todo in mine if todo.label in ready or todo._doc["state"] == "running"]
    problems = []
    if len(forked) < 2:
        problems.append(f"fork_join needs at least two todos that can run side by side; {len(forked)} found")
    for todo in mine:
        if todo.label in plan._inline:
            problems.append(f"{todo.key} is inline and writes this kernel's workspace; delegate it")
        elif not _reads_only(todo) and _delegation(todo).get("spec", {}).get("isolation") != "worktree":
            problems.append(f"{todo.key} writes the shared workspace; use Writer(isolation='worktree') or a Reader")
    if problems:
        raise Geometry("fork_join", problems)
    await _schedule(plan, run, {todo.label for todo in mine}, restart=True)


def _scatter_geometry(plan: Plan) -> tuple[list[Todo], Todo]:
    readers = [
        todo
        for todo in plan.todos
        if _reads_only(todo) and not ROUND.search(todo.key) and todo._doc["state"] != "abandoned"
    ]
    leads = [todo for todo in plan.todos if todo.label in plan._inline]
    problems = []
    if len(readers) < 2:
        problems.append(f"scatter needs at least two readers; {len(readers)} found")
    bound: dict[str, str] = {}
    for todo in readers:
        partition = _delegation(todo).get("context") or []
        if not partition:
            problems.append(f"reader {todo.key} is bound to no partition")
        for url in partition:
            for other, key in bound.items():
                if _inside(url, other) or _inside(other, url):
                    problems.append(f"readers {key} and {todo.key} share {url}")
        bound.update(dict.fromkeys(partition, todo.key))
        items = (todo._doc.get("contract") or {}).get("items", [])
        if not any("schema" in item["decider"] for item in items):
            problems.append(f"reader {todo.key} declares no answer schema, so its answer is never stored")
    if len(leads) != 1:
        problems.append(f"scatter needs exactly one lead, an inline todo declared with run=; {len(leads)} found")
    elif leads[0]._doc["state"] != "pending":
        problems.append(f"lead {leads[0].key} is {leads[0]._doc['state']}; scatter starts a pending lead")
    if problems:
        raise Geometry("scatter", problems)
    return readers, leads[0]


async def _asked(plan: Plan, reader: Todo, number: int, question: str | None, context: list = ()) -> Todo:
    """The reader's todo for this round: itself in the first, a sibling declared once after.

    ``context`` is added to the sibling's own; a pod hands its arbiter the readers' answers so.
    """
    if number == 1:
        return reader
    key = f"{reader.key}-r{number}"
    held = next((todo for todo in plan.todos if todo.key == key), None)
    if held is not None:
        return held
    doc = reader._doc
    # Invariant: the key alone is the label, because a label is capped at 80 characters and
    # may hold no newline; the lead writes the question and it rides the delegation's note.
    wire = {
        "label": key,
        "delegation": {
            **doc["delegation"],
            "note": question,
            "context": [*doc["delegation"].get("context", []), *context],
        },
        "contract": doc.get("contract"),
    }
    await plan._op("append", {"todos": [_pruned(wire)]})
    return Todo(plan, wire["label"])


async def _survivor(todo: Todo) -> dict[str, Any] | None:
    """A reader's answer with the quotes its own partition bears out, or None when it is dropped.

    Invariant: the lead never sees an abstention, a failed reader, an answer none of whose
    quotes verified, or a quote from outside the partition this reader was bound to; a failed
    reader's todo is abandoned so the plan can still finish.
    """
    if todo._doc["state"] == "failed":
        try:
            await todo.retry()
            await todo._op("drop")
        except PlanError:
            pass
    if todo._doc["state"] != "done" or not todo._doc.get("output"):
        return None
    try:
        said = json.loads(await rlm.fetch(todo._doc["output"], as_text=True))
    except (RuntimeError, ValueError):
        return None
    if not isinstance(said, dict) or said.get("answer") is None:
        return None
    quotes = await verify_quotes(said.get("quotes"), _delegation(todo).get("context") or ())
    return {"reader": todo.key, "answer": said["answer"], "quotes": quotes} if quotes else None


async def scatter(plan: Plan, run: Run) -> None:
    """Ask readers bound to disjoint partitions, in rounds, until the lead commits.

    Declare each reader with ``Reader(partition=[...])`` and ``schema(shapes.ANSWER,
    critical=True)``, and one lead as an inline todo: ``async def lead(answers,
    number)`` returning ``{"commit": answer}`` or ``{"ask": question}``. The lead
    sees only ``{"reader", "answer", "quotes"}`` whose quotes ``verify_quotes``
    found on the cited line inside that reader's own partition. A commit becomes
    the lead todo's product, ``{"answer", "rounds"}``, and goes to ``done``;
    ``SCATTER_MAX_ROUNDS`` rounds without one fail the lead.

        run = await plan.run(shape=scatter, budget="20m")
    """
    readers, lead = _scatter_geometry(plan)
    ask = plan._inline[lead.label]
    await lead.start()
    question = None
    for number in range(1, SCATTER_MAX_ROUNDS + 1):
        batch = [await _asked(plan, reader, number, question) for reader in readers]
        await _schedule(plan, run, {todo.label for todo in batch})
        if run.over:
            cause = "stopped before the lead committed"
            break
        answers = [answer for todo in batch if (answer := await _survivor(todo))]
        try:
            said = await ask(answers, number) or {}
        except Exception as error:
            cause = f"the lead raised {error!r}"
            break
        if "commit" in said:
            await run._complete(lead, {"answer": said["commit"], "rounds": number})
            return
        question = said.get("ask")
        if not question:
            cause = "the lead neither committed nor asked"
            break
    else:
        cause = f"no commit within {SCATTER_MAX_ROUNDS} rounds"
    await lead.fail(cause)
