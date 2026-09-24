"""The review pod: readers with distinct briefs, and one arbiter whose command is the verdict.

A reader's finding is evidence and never a vote. The arbiter is declared like any
todo and issued again once the readers settle, as ``<key>-r2``, because a declared
todo's note cannot change: the issue carries the findings in its delegation's note
(1 KiB, the host's cap) and every reader's full answer as context.
"""
from __future__ import annotations

from typing import Any

from ..contract import Contract, Item, contract, schema
from ..plan import Plan, Run, Todo
from ..roles import Reader, Role, Writer
from ..shapes import ANSWER, ROUND, Geometry, _abandon, _asked, _delegation, _reads_only, _schedule, _survivor

NOTE_MAX = 1024
BRIEFS = {
    "correctness": "Review for behavior that is wrong: a broken invariant, an unhandled input, a changed contract.",
    "tests": "Review the tests: what the change does that no test would notice breaking.",
    "scope": "Review the scope: what the change touches that its stated goal does not need.",
}
ASKED = " Answer null when you find nothing; otherwise quote the line that shows it."


async def declare(plan: Plan, subject: list | tuple, check: Item | Contract, arbiter: Role | None = None) -> Todo:
    """Declare a pod on ``plan``: one ``Reader`` per brief in ``BRIEFS`` over ``subject``, then the arbiter.

    ``check`` is the arbiter's ``cmd`` or ``example`` item (or a whole contract); the
    arbiter is a ``Writer`` in its own worktree unless ``arbiter`` names another role.

        await declare(plan, ["local://src"], cmd("make -s check", critical=True))
    """
    answer = contract(schema(ANSWER, critical=True))
    passes = [
        await plan.todo(key=f"read-{key}", delegate=Reader(partition=subject, note=brief + ASKED), accept=answer)
        for key, brief in BRIEFS.items()
    ]
    accept = check if isinstance(check, Contract) else contract(check)
    return await plan.todo(key="arbiter", after=passes, delegate=arbiter or Writer(accept=accept), accept=accept)


def _geometry(plan: Plan) -> tuple[list[Todo], Todo]:
    declared = [todo for todo in plan.todos if not ROUND.search(todo.key)]
    issued = {todo.key for todo in plan.todos if ROUND.search(todo.key)}
    readers = [todo for todo in declared if _reads_only(todo)]
    arbiters = [
        todo
        for todo in declared
        if not _reads_only(todo) and (todo._doc["state"] != "abandoned" or f"{todo.key}-r2" in issued)
    ]
    problems = []
    if len(readers) < 2:
        problems.append(f"a pod needs at least two readers; {len(readers)} found")
    briefs = [_delegation(todo).get("note") for todo in readers]
    if None in briefs or len(set(briefs)) < len(briefs):
        problems.append("every reader needs a brief of its own in its role's note")
    for todo in readers:
        items = (todo._doc.get("contract") or {}).get("items", [])
        if not any("schema" in item["decider"] for item in items):
            problems.append(f"reader {todo.key} declares no answer schema, so its finding is never stored")
    if len(arbiters) != 1:
        problems.append(f"a pod needs exactly one arbiter, the one todo that is not a reader; {len(arbiters)} found")
    for todo in arbiters:
        items = (todo._doc.get("contract") or {}).get("items", [])
        if not _delegation(todo):
            problems.append(f"arbiter {todo.key} has no delegate to hand the findings to")
        if not any(item["critical"] and ("cmd" in item["decider"] or "example" in item["decider"]) for item in items):
            problems.append(f"arbiter {todo.key} needs a critical cmd or example item: code decides a pod")
        # Invariant: a judged arbiter would meet no floor and spend the todo's juries on one verdict.
        if any("judge" in item["decider"] for item in items):
            problems.append(f"arbiter {todo.key} carries a judge item; a pod's verdict is a command's")
    if problems:
        raise Geometry("review_pod", problems)
    return readers, arbiters[0]


def _evidence(findings: list[tuple[Todo, dict[str, Any] | None]]) -> str:
    lines = ["Readers' findings, as evidence; the contract's command decides."]
    for todo, found in findings:
        cited = ", ".join(f"{quote['url']}:{quote['line']}" for quote in (found or {}).get("quotes", []))
        lines.append(f"{todo.key}: {found['answer']} [{cited}]" if found else f"{todo.key}: no backed finding")
    body = "\n".join(lines).encode("utf-8")
    if len(body) <= NOTE_MAX:
        return body.decode("utf-8")
    # A finding cut in half reads as a different finding, so the cut says so and the whole
    # answer is one fetch away in the context urls.
    said = f"\n[cut at {NOTE_MAX} bytes; every reader's whole answer is in this todo's context]"
    room = NOTE_MAX - len(said.encode("utf-8"))
    return body[:room].decode("utf-8", errors="ignore") + said


async def review_pod(plan: Plan, run: Run) -> None:
    """Run a review pod: every reader first, then one arbiter whose ``cmd`` or ``example`` is the verdict.

    Refused with ``Geometry`` unless the plan holds two or more ``Reader`` todos with
    distinct notes, answering ``shapes.ANSWER``, and exactly one other delegated todo
    with a critical ``cmd`` or ``example`` item and no judged one (``declare`` in
    ``yi.recipes.review_pod`` builds that). A finding survives only with a quote
    ``verify_quotes`` found inside the reader's partition. The arbiter runs as
    ``<key>-r2`` with the findings in its note and is verified once, never retried.

        run = await plan.run(shape=review_pod, budget="30m")
    """
    readers, arbiter = _geometry(plan)
    # The declared arbiter is the issue's template: held, or the engine starts it without findings.
    # It waits on a reader, not on the user: a user block would ask the owner a question nobody posed.
    if arbiter._doc["state"] == "pending":
        reader = readers[0]
        await arbiter.block({"child": reader.child or reader.key}, f"the pod issues it as {arbiter.key}-r2 with the findings")
    await _schedule(plan, run, {todo.label for todo in readers}, restart=True)
    if run.over:
        return
    findings = [(todo, await _survivor(todo)) for todo in readers]
    outputs = [todo._doc["output"] for todo, _ in findings if todo._doc.get("output")]
    issued = await _asked(plan, arbiter, 2, _evidence(findings), outputs)
    await _abandon(readers, issued)
    if arbiter._doc["state"] in ("pending", "blocked"):
        await arbiter._op("drop")
    await _schedule(plan, run, {issued.label})
