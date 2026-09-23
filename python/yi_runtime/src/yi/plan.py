"""Plans as programs: ``Plan``, ``Todo`` and ``Run`` over the host's ``plan.op``.

The library holds no state the store does not: every method is one host
request, and the host decides admission, step legality and verification.
"""
from __future__ import annotations

import asyncio
import re
import uuid
from typing import Any, Awaitable, Callable

import rlm

from .contract import JSON_TYPE, Contract, canonical, freeze
from .roles import Role

KEY = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
SOURCE_TYPE = "text/x-python"
READ_ONLY = ("view", "repair")
WAIT_SECONDS = 300.0
REAPED_POLL = 0.5

# The cell the kernel is executing: its id, its source as submitted, the plans it touched.
_CELL: dict[str, Any] | None = None
# Invariant: one scheduler per plan in this kernel; the lease dies with the kernel that held it.
_RUNS: dict[str, "Run"] = {}


class PlanError(RuntimeError):
    """The host refused an op; ``code`` and ``kind`` say which refusal.

        try:
            await todo.start()
        except PlanError as refusal:
            print(refusal.kind, refusal)
    """

    def __init__(self, message: str, refusal: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.refusal = refusal or {}
        self.code = self.refusal.get("code", "")
        self.kind = self.refusal.get("kind", self.code)


class Refused(PlanError):
    """``done`` ran the contract and it did not pass; ``verdict`` has every item's line.

        try:
            await todo.done()
        except Refused as refusal:
            print(refusal.verdict["outcome"])
    """

    @property
    def verdict(self) -> dict[str, Any]:
        return self.refusal.get("verdict") or {}


class Stale(PlanError):
    """``done`` verified an attempt the todo has since moved past; start again.

        try:
            await todo.done()
        except Stale:
            await plan.refresh()
    """


class SpecDrift(PlanError):
    """A redeclared todo differs from the one the plan holds; nothing was written.

    ``diff`` maps each changed field to ``(stored, declared)``. The road on is
    ``todo.retry(delegate=...)`` after a failure, or ``plan.supersede(reason)``.

        try:
            await plan.todo(key="tests", label="write the test suite")
        except SpecDrift as drift:
            print(drift.key, sorted(drift.diff))
    """

    def __init__(self, key: str, diff: dict[str, tuple]) -> None:
        super().__init__(f"todo {key!r} is already declared differently: {', '.join(sorted(diff))}")
        self.key = key
        self.diff = diff


class RunActive(PlanError):
    """The plan already has a scheduler running another shape; ``run_id`` names it.

        try:
            await plan.run(budget="10m")
        except RunActive as active:
            print(active.run_id)
    """

    def __init__(self, run_id: str) -> None:
        super().__init__(f"run {run_id} already schedules this plan; stop it or attach with its shape")
        self.run_id = run_id


def _on_cell(info: Any) -> None:
    global _CELL
    _CELL = {"id": uuid.uuid4().hex[:12], "source": getattr(info, "raw_cell", "") or "", "plans": set()}


def _install() -> None:
    shell = rlm.get_ipython() if rlm.get_ipython is not None else None
    if shell is not None:
        shell.events.register("pre_run_cell", _on_cell)


def _raise(reply: dict[str, Any]) -> None:
    refusal = reply.get("refusal") or {}
    kinds = {"refused": Refused, "stale": Stale}
    raise kinds.get(refusal.get("kind"), PlanError)(refusal.get("message", "plan.op refused"), refusal)


async def _send(op: str, args: dict | None, **kwargs: Any) -> dict[str, Any]:
    request_id = kwargs.pop("request_id", None) or str(uuid.uuid4())
    try:
        reply = await rlm.plan_op(op, args, request_id=request_id, **kwargs)
    except RuntimeError as error:
        # The one host error that leaves the commit unknown; the same id makes the retry safe.
        if "task failed" not in str(error):
            raise
        reply = await rlm.plan_op(op, args, request_id=request_id, **kwargs)
    if not reply.get("ok"):
        _raise(reply)
    return reply


async def _record(plan_id: str) -> None:
    """Invariant: the cell's source is in the journal before the cell's first effect on a plan."""
    cell = _CELL
    if cell is None or plan_id in cell["plans"] or not cell["source"].strip():
        return
    ref, blob = freeze(cell["source"], SOURCE_TYPE)
    try:
        await _send("program", {"cell_id": cell["id"], "source_ref": ref}, plan=plan_id, artifacts=[blob])
    except PlanError as refusal:
        # A child's cells are its own session's, never the plan's program: the host takes the
        # record from the owner alone, so a submitting child records nothing and stops asking.
        if refusal.code != "not_owner":
            raise
    cell["plans"].add(plan_id)


def _pruned(value: Any) -> Any:
    """Without the keys the host omits when it writes a todo back: None and empty."""
    if isinstance(value, dict):
        kept = {key: _pruned(item) for key, item in value.items()}
        return {key: item for key, item in kept.items() if item not in (None, [], {})}
    if isinstance(value, list):
        return [_pruned(item) for item in value]
    return value


def _product(plan_id: str, value: Any) -> tuple[str, dict]:
    """A product's url in the plan's store, named by content, and the blob behind it."""
    ref, blob = freeze(value if isinstance(value, str) else canonical(value), JSON_TYPE)
    return f"plan://{plan_id}/artifacts/{ref['digest'].removeprefix('sha256:')}", blob


def _seconds(budget: float | str | None) -> float | None:
    if budget is None or isinstance(budget, (int, float)):
        return budget
    match = re.fullmatch(r"(\d+(?:\.\d+)?)([smh])", budget.strip())
    if not match:
        raise ValueError(f"budget {budget!r} is not seconds or a span like '90s', '30m', '2h'")
    return float(match.group(1)) * {"s": 1, "m": 60, "h": 3600}[match.group(2)]


class Todo:
    """One todo of a plan; every method is one host op the step table may refuse.

        todo = plan["freeze"]
        await todo.start()
    """

    def __init__(self, plan: "Plan", label: str) -> None:
        self.plan = plan
        self.label = label

    def __repr__(self) -> str:
        return f"Todo({self.key!r}, {self._doc.get('state')!r})"

    @property
    def key(self) -> str:
        # ponytail: the key rides the label (`key: text`) because TodoSpec has no field for one
        # and thirty-five construction sites; give it a field when a second surface needs keys.
        head = self.label.split(": ", 1)[0]
        return head if KEY.match(head) else self.label

    @property
    def _doc(self) -> dict[str, Any]:
        for todo in self.plan._doc.get("todos", []):
            if todo.get("label") == self.label:
                return todo
        raise KeyError(f"no todo {self.label!r} in plan {self.plan.id}")

    @property
    def attempt(self) -> int:
        return self._doc.get("attempt", 1)

    @property
    def child(self) -> str | None:
        """The agent running this todo, or None for an inline or idle one."""
        by = self._doc.get("by")
        return by if by and self._doc.get("delegation") else None

    async def state(self) -> str:
        """Refresh the plan and return this todo's state name.

            print(await todo.state())
        """
        await self.plan.refresh()
        return self._doc["state"]

    async def _op(self, op: str, **args: Any) -> "Todo":
        await self.plan._op(op, {"label": self.label, **_pruned(args)})
        return self

    async def start(self) -> "Todo":
        """Step your own todo to running; the engine starts a delegated one itself.

            await todo.start()
        """
        return await self._op("start")

    async def submit(self, artifact: Any) -> str:
        """Record this attempt's product in the plan's store and return its url.

            url = await todo.submit({"passed": 12})
        """
        url, blob = _product(self.plan.id, artifact)
        await self.plan._op(
            "submit", {"label": self.label, "attempt": self.attempt, "output": url}, artifacts=[blob]
        )
        return url

    async def done(self, output: str | None = None) -> "Todo":
        """Ask the host to verify and complete; raises ``Refused`` or ``Stale``.

        ``output`` is a url; a submitted product is used when it is omitted.

            await todo.done()
        """
        return await self._op("done", output=output or self._doc.get("submitted"))

    async def fail(self, cause: str) -> "Todo":
        """Mark this attempt failed; ``retry`` opens the next one.

            await todo.fail("the fixture server never came up")
        """
        return await self._op("fail", cause=cause)

    async def block(self, on: Any, note: str) -> "Todo":
        """Park the todo on ``"user"``, ``{"child": name}`` or ``{"external": {...}}``.

            await todo.block("user", "which region should this deploy to?")
        """
        return await self._op("block", on=on, note=note)

    async def unblock(self) -> "Todo":
        """Return a blocked todo to pending.

            await todo.unblock()
        """
        return await self._op("unblock")

    async def retry(self, delegate: Role | None = None) -> "Todo":
        """Open a new attempt after a failure, optionally with another delegate.

        The todo keeps its contract; a new delegate's ``accept`` is not read here.

            await todo.retry(delegate=Writer(accept=contract(cmd("make -s check", critical=True)), model="strong"))
        """
        return await self._op("retry", delegation=delegate.delegation() if delegate else None)

    async def decompose(self, todos: list[dict[str, Any]]) -> "Plan":
        """Open a sub-plan under this todo; each dict takes ``plan.todo``'s arguments.

            sub = await todo.decompose([{"key": "parse"}, {"key": "emit", "after": ["parse"]}])
        """
        specs, blobs, among = [], [], {}
        for spec in todos:
            wire, more, key = await self.plan._spec(**spec, among=among)
            among[key] = wire["label"]
            specs.append(wire)
            blobs.extend(more)
        await self.plan._op("decompose", {"label": self.label, "todos": specs}, artifacts=blobs)
        return await Plan.attach(self._doc["subplan"])

    async def result(self, schema: dict | None = None, timeout: float = 540.0) -> dict[str, Any]:
        """Wait for this todo's child and return its answer (``rlm.result``).

        A child the host calls stuck is still waited on: one long tool call is not a
        failure, and neither is one that asked you something and kept working.

            answer = await todo.result(timeout=120)
        """
        child = self.child
        if child is None:
            raise PlanError(f"todo {self.key!r} has no child; only a running delegated todo does")
        loop = asyncio.get_running_loop()
        deadline, cursor = loop.time() + timeout, 0
        while (remaining := deadline - loop.time()) > 0:
            reply = await rlm.wait(timeout=min(remaining, WAIT_SECONDS), cursor=cursor)
            cursor = reply.get("cursor", cursor)
            state = (reply.get("states") or {}).get(child)
            if state is None:
                raise PlanError(f"child {child} is not registered with this session")
            if state in ("finished", "failed", "needs_you"):
                try:
                    return await rlm.result(child, schema=schema)
                except RuntimeError as error:
                    # needs_you also names a child still running with a question for you.
                    if state != "needs_you" or "still running" not in str(error):
                        raise
        raise TimeoutError(f"child {child} did not finish within {timeout}s")

    async def cancel(self) -> "Todo":
        """Stop this todo: a pending one is dropped, a running one failed, which stops its child.

        An inline coroutine is cancelled and what it submitted is kept.

            await todo.cancel()
        """
        state = await self.state()
        if state != "running":
            return await self._op("drop")
        task = self.plan._tasks.pop(self.label, None)
        if task is not None:
            task.cancel()
        # A delegated child is reaped by the fail itself: an interrupt first would hand its end
        # to the engine, which fails the todo before this call can.
        return await self.fail("cancelled")


class Plan:
    """A plan in the host's store: ``create`` opens one, ``attach`` and ``resume`` join one.

        plan = await Plan.create("ship logrotate-lite with a packaged tarball")
        freeze = await plan.todo(key="freeze", accept=contract(cmd("make -s check", critical=True)))
    """

    def __init__(self, doc: dict[str, Any], notices: list[str] | None = None) -> None:
        self._doc = doc
        self._notices = notices or []
        # What the last view said the engine left standing: held by the width, a start it was
        # refused and a finish it left running, each by label with the engine's reason.
        self._held: list[str] = []
        self._unstarted: dict[str, str] = {}
        self._engine_left: dict[str, str] = {}
        self._inline: dict[str, Callable[[], Awaitable[Any]]] = {}
        self._tasks: dict[str, asyncio.Task] = {}

    def __repr__(self) -> str:
        return f"Plan({self.id!r}, revision={self.revision})"

    @property
    def id(self) -> str:
        return self._doc["plan"]

    @property
    def goal(self) -> str:
        return self._doc["goal"]

    @property
    def revision(self) -> int:
        return self._doc.get("touched", 0)

    @property
    def todos(self) -> list[Todo]:
        return [Todo(self, todo["label"]) for todo in self._doc.get("todos", [])]

    def __getitem__(self, name: str) -> Todo:
        for todo in self.todos:
            if name in (todo.key, todo.label, todo.label.split(": ", 1)[-1]):
                return todo
        raise KeyError(f"no todo {name!r} in plan {self.id}")

    def ready(self) -> list[Todo]:
        """The todos the host says may start now, as of the last reply.

            for todo in plan.ready():
                print(todo.key)
        """
        return [Todo(self, label) for label in self._doc.get("ready", [])]

    @property
    def unresolved(self) -> list[Todo]:
        """Running todos nobody can vouch for; never retried without a decision.

        The host names a delegated todo whose child left no durable result (the
        user settles it with ``yi plan repair``); an inline todo whose coroutine
        died with its kernel is settled by ``todo.fail(cause)`` and ``todo.retry()``.
        """
        # ponytail: the host's findings arrive as notice prose, so this matches their prefix;
        # read a field instead once `repair` answers with its findings typed.
        out = []
        for todo in self.todos:
            doc = todo._doc
            if doc.get("state") != "running":
                continue
            flagged = any(note.startswith(f"{self.id}/{todo.label} needs reconciliation") for note in self._notices)
            orphan = not doc.get("delegation") and todo.label not in self._tasks
            if flagged or orphan:
                out.append(todo)
        return out

    @classmethod
    async def create(cls, goal: str, request_id: str | None = None) -> "Plan":
        """Open a new plan. Pass a ``request_id`` and a re-run cell opens the same plan.

        It never guesses that an open plan with similar text is this one.

            plan = await Plan.create("ship logrotate-lite", request_id="create-01")
        """
        reply = await _send("init", {"goal": goal, "todos": []}, request_id=request_id)
        plan = cls(reply["plan"])
        await _record(plan.id)
        return plan

    @classmethod
    async def attach(cls, plan_id: str) -> "Plan":
        """Join an existing plan by id and read it as it stands.

            plan = await Plan.attach("ship-logrotate-lite")
        """
        return cls((await _send("view", {"full": True}, plan=plan_id))["plan"])

    @classmethod
    async def resume(cls, plan_id: str) -> "Plan":
        """Reattach after a restart: durable state only, no saved source is ever run.

        Done todos keep their results, live children are waited on again by
        ``run``, and ``plan.unresolved`` lists the attempts that need a decision.

            plan = await Plan.resume("ship-logrotate-lite")
        """
        reply = await _send("repair", None, plan=plan_id)
        return cls(reply["plan"], reply.get("notices"))

    async def refresh(self) -> "Plan":
        """Read the plan again; handles stay valid.

            await plan.refresh()
        """
        reply = await _send("view", {"full": True}, plan=self.id)
        self._doc, self._held = reply["plan"], reply.get("held") or []
        self._unstarted, self._engine_left = reply.get("unstarted") or {}, reply.get("left") or {}
        return self

    async def _op(self, op: str, args: dict | None = None, **kwargs: Any) -> dict[str, Any]:
        if op not in READ_ONLY:
            await _record(self.id)
        reply = await _send(op, args, plan=self.id, **kwargs)
        self._doc = reply["plan"] if reply["plan"].get("plan") == self.id else self._doc
        return reply

    async def _spec(
        self,
        key: str,
        label: str | None = None,
        *,
        after: list | tuple = (),
        delegate: Role | None = None,
        accept: Contract | None = None,
        run: Callable[[], Awaitable[Any]] | None = None,
        among: dict[str, str] | None = None,
    ) -> tuple[dict, list[dict], str]:
        if not isinstance(key, str) or not KEY.match(key):
            raise ValueError(f"key {key!r} must match {KEY.pattern}")
        if run is not None and (delegate is not None or not callable(run)):
            raise TypeError("run= takes an async function, and an inline todo has no delegate")
        # A sub-plan's todos name each other before any of them exists: `among` is that batch.
        edges = [item.label if isinstance(item, Todo) else (among or {}).get(item) or self[item].label for item in after]
        if accept is None and delegate is not None:
            accept = delegate.accept
        if accept is None and delegate is not None and delegate.isolation == "worktree":
            raise TypeError("a worktree Writer needs accept=")
        contract, blobs = None, []
        if accept is not None:
            contract_class = delegate.contract_class if delegate else "inline"
            contract, blobs = await accept.render(contract_class, lambda url: rlm.fetch(url, as_text=True))
        delegation, noted = delegate.note_blobs() if delegate else (None, [])
        wire = {
            "label": f"{key}: {label}" if label else key,
            "after": edges,
            "delegation": delegation,
            "contract": contract,
        }
        return _pruned(wire), blobs + noted, key

    async def todo(self, key: str, label: str | None = None, **spec: Any) -> Todo:
        """Declare a todo once: ``after``, ``delegate``, ``accept`` and ``run`` say what it is.

        The same declaration again returns the handle and writes nothing; a
        changed one raises ``SpecDrift`` and writes nothing. ``run=`` names an
        async function executed in this kernel; ``delegate=`` a child's role,
        whose ``accept`` is the contract unless ``accept=`` names another.

            tests = await plan.todo(key="tests", label="write the test suite", after=["freeze"],
                delegate=Writer(accept=contract(cmd("pytest -q tests/", critical=True))))
        """
        wire, blobs, key = await self._spec(key, label, **spec)
        for _ in range(2):
            await self.refresh()
            held = next((todo for todo in self.todos if todo.key == key), None)
            if held is not None:
                stored = _pruned(held._doc)
                fields = ("label", "after", "delegation", "contract")
                diff = {name: (stored.get(name), wire.get(name)) for name in fields if stored.get(name) != wire.get(name)}
                if diff:
                    raise SpecDrift(key, diff)
                break
            try:
                await self._op("append", {"todos": [wire]}, artifacts=blobs, expected_revision=self.revision)
                held = Todo(self, wire["label"])
                break
            except PlanError as refusal:
                if refusal.kind != "stale_revision":
                    raise
        else:
            raise PlanError(f"plan {self.id} kept moving while {key!r} was declared; try again")
        if spec.get("run") is not None:
            self._inline[held.label] = spec["run"]
        return held

    async def supersede(self, reason: str) -> "Plan":
        """Close this generation of todos and open the next; declare the new ones after.

            await plan.supersede("the CLI surface changed under the plan")
        """
        await self._op("supersede", {"reason": reason, "todos": []})
        return self

    async def run(
        self,
        shape: Callable[["Plan", "Run"], Awaitable[None]] | None = None,
        budget: float | str | None = None,
        detach: bool = False,
    ) -> "Run":
        """Schedule the plan until it finishes, the shape returns, or ``budget`` runs out.

        One scheduler per plan: a second call with the same shape attaches to
        the running one, another shape raises ``RunActive``. ``detach=True``
        returns at once and the scheduler keeps stepping between cells
        (experimental). Read ``run.outcome`` for how it ended.

            run = await plan.run(budget="2h")
            print(run.outcome)
        """
        shape = shape or in_order
        held = _RUNS.get(self.id)
        if held is not None and held.outcome is None:
            if held.shape is not shape:
                raise RunActive(held.id)
        else:
            held = _RUNS[self.id] = Run(self, shape, _seconds(budget))
            held._task = asyncio.ensure_future(held._drive())
        if not detach:
            await held
        return held


class Run:
    """One scheduler's lease on a plan; ``outcome`` is None while it runs.

    It ends ``verified_success``, ``accepted_by_user``, ``failed``,
    ``cancelled`` or ``unresolved``. A shape drives it with ``launch``,
    ``settle`` and ``over``.

        run = await plan.run(budget="30m", detach=True)
        print(await run.status())
    """

    def __init__(self, plan: Plan, shape: Callable, budget: float | None) -> None:
        self.id = uuid.uuid4().hex[:12]
        self.plan = plan
        self.shape = shape
        self.outcome: str | None = None
        self.refusals: dict[str, PlanError] = {}
        self.active: dict[str, Todo] = {}
        self._left: set[tuple[str, Any]] = set()
        self._deadline = None if budget is None else asyncio.get_running_loop().time() + budget
        self._stop = asyncio.Event()
        self._cancelled = False
        self._waiting: asyncio.Task | None = None
        self._states: dict[str, str] = {}
        self._reaped = False
        # Cursor 0, not a bare wait: the model's own bare waits may have seen a child end already.
        self._cursor = 0
        self._task: asyncio.Task | None = None

    def __await__(self):
        return self._task.__await__()

    def _remaining(self) -> float:
        if self._deadline is None:
            return WAIT_SECONDS
        return min(WAIT_SECONDS, self._deadline - asyncio.get_running_loop().time())

    @property
    def over(self) -> bool:
        """True once the run was stopped or its budget ran out; a shape checks it each round."""
        return self._stop.is_set() or self._remaining() <= 0

    async def launch(self, todo: Todo) -> PlanError | None:
        """Start one todo; returns the host's refusal instead of raising it.

        The engine starts a delegated todo once admission lets it, so launching one
        adopts it; a refusal of kind ``admission`` means wait for a slot and try again,
        any other is the engine's own refusal to start it.

            refusal = await run.launch(plan["tests"])
        """
        if todo._doc.get("delegation"):
            if (todo.label, todo._doc.get("attempt")) in self._left:
                return PlanError(f"the engine left {todo.key} running for you", {"code": "left"})
            if await todo.state() != "running":
                if todo.label in self.plan._held or todo.label not in self.plan._doc.get("ready", []):
                    return PlanError(f"{todo.key} waits on the dispatch width", {"code": "admission"})
                said = self.plan._unstarted.get(todo.label)
                message = f"the engine could not start {todo.key}: {said}" if said else f"the engine has not started {todo.key}"
                refusal = PlanError(message, {"code": "not_started"})
                self.refusals[todo.key] = refusal
                return refusal
        else:
            try:
                await todo.start()
            except PlanError as refusal:
                self.refusals[todo.key] = refusal
                return refusal
        self.active[todo.label] = todo
        inline = self.plan._inline.get(todo.label)
        if inline is not None:
            self.plan._tasks[todo.label] = asyncio.ensure_future(inline())
        return None

    async def _complete(self, todo: Todo, product: Any) -> None:
        """An inline todo's ending; the engine completes a delegated one from its child's finish."""
        try:
            await todo.done(await todo.submit(product) if product is not None else None)
        except PlanError as refusal:
            self.refusals[todo.key] = refusal
            # Invariant: only a verdict on the product fails the attempt; an abstention is an
            # infrastructure failure and an escalation is a question (plan section 6.3).
            if isinstance(refusal, Refused) and refusal.verdict.get("outcome") == "fail":
                await todo.fail(f"done refused: {refusal}"[:500])

    async def settle(self) -> list[Todo]:
        """Wait until an active attempt ends, then collect it.

        A returned coroutine goes to ``done`` (the contract decides), a raised one
        to ``fail``. The engine submits and accepts, refuses or fails a delegated
        todo when its child ends, so the state is read, never written; a stuck or
        reaped child is waited on, and only a child asking you something blocks its todo.

            settled = await run.settle()
        """
        tasks = {self.plan._tasks[label]: label for label in self.active if label in self.plan._tasks}
        children = {todo.label: todo.child for todo in self.active.values() if todo.child}
        if children and self._waiting is None:
            self._waiting = asyncio.ensure_future(rlm.wait(timeout=self._remaining(), cursor=self._cursor))
        stop = asyncio.ensure_future(self._stop.wait())
        waiters = [*tasks, stop, *([self._waiting] if self._waiting else [])]
        # A reaped child moves nothing a wait sees, so its todo is re-read on a short poll.
        timeout = min(self._remaining(), REAPED_POLL) if self._reaped else self._remaining()
        done, _ = await asyncio.wait(waiters, timeout=timeout, return_when=asyncio.FIRST_COMPLETED)
        stop.cancel()
        settled = []
        for task in done & set(tasks):
            todo = self.active.pop(tasks[task])
            self.plan._tasks.pop(todo.label, None)
            if task.cancelled() or task.exception() is not None:
                await todo.fail("cancelled" if task.cancelled() else f"inline todo raised {task.exception()!r}")
            else:
                await self._complete(todo, task.result())
            settled.append(todo)
        if self._waiting in done:
            reply, self._waiting = self._waiting.result(), None
            self._cursor = reply.get("cursor", self._cursor)
            self._states = reply.get("states") or {}
        elif not self._reaped:
            return settled
        self._reaped = False
        for label, child in children.items():
            state = self._states.get(child)
            if state in ("queued", "running", "stuck"):
                continue
            todo = self.active[label]
            now = await todo.state()
            if now == "running" and state == "needs_you":
                await todo.block("user", f"child {child} is {state}")
            elif now == "running":
                # Invariant: the engine settles a delegated todo, so a finish in flight or a reaped
                # child is waited on; only the engine's word that it left the todo collects it.
                if state not in ("finished", "failed") or label not in self.plan._engine_left:
                    self._reaped |= state is None
                    continue
                self._left.add((label, todo._doc.get("attempt")))
            self.active.pop(label)
            settled.append(todo)
        return settled

    async def _drive(self) -> "Run":
        plan = self.plan
        try:
            fresh = await Plan.resume(plan.id)
            plan._doc, plan._notices = fresh._doc, fresh._notices
            held = {todo.label for todo in plan.unresolved}
            # Reconnect live work: a running child the host vouches for is waited on, not respawned.
            self.active = {t.label: t for t in plan.todos if t._doc["state"] == "running" and t.label not in held}
            await self.shape(plan, self)
        finally:
            if self._waiting is not None:
                self._waiting.cancel()
            # An inline coroutine has no one to collect it once the scheduler is gone.
            for todo in list(self.active.values()):
                if self._cancelled or todo.label in plan._tasks:
                    await todo.cancel()
            await plan.refresh()
            self.outcome = self._outcome()
            _RUNS.pop(plan.id, None)
        return self

    def _outcome(self) -> str:
        docs = [todo._doc for todo in self.plan.todos if todo._doc["state"] != "abandoned"]
        if any(doc["state"] not in ("done", "failed") for doc in docs):
            return "cancelled" if self._cancelled else "unresolved"
        if any(doc["state"] == "failed" for doc in docs):
            return "failed"
        resolutions = {doc.get("resolution") for doc in docs}
        if resolutions <= {"verified_done", "accepted_by_user"}:
            return "accepted_by_user" if "accepted_by_user" in resolutions else "verified_success"
        return "unresolved"

    async def status(self) -> dict[str, Any]:
        """The run as data: its outcome, every todo's state, what is active or unresolved.

            print(await run.status())
        """
        await self.plan.refresh()
        return {
            "run": self.id,
            "outcome": self.outcome,
            "states": {todo.key: todo._doc["state"] for todo in self.plan.todos},
            "active": [todo.key for todo in self.active.values()],
            "unresolved": [todo.key for todo in self.plan.unresolved],
            "refusals": {key: str(refusal) for key, refusal in self.refusals.items()},
        }

    async def stop(self, scope: str = "scheduling") -> dict[str, Any]:
        """Stop scheduling; ``scope="cancel_active"`` also cancels what is running.

        Returns ``status()``, which reports what remains.

            print(await run.stop())
        """
        if scope not in ("scheduling", "cancel_active"):
            raise ValueError("scope is 'scheduling' or 'cancel_active'")
        self._cancelled = scope == "cancel_active"
        self._stop.set()
        if self._task is not None:
            await self._task
        return await self.status()


async def in_order(plan: Plan, run: Run) -> None:
    """The default shape: settle what the engine starts, start ready ``run=`` todos, repeat.

    The host's admission count bounds the width; a todo with neither a
    delegate nor ``run=`` is the owner's own and is left alone.

        run = await plan.run(shape=in_order, budget="1h")
    """
    while not run.over:
        await plan.refresh()
        unresolved = {todo.label for todo in plan.unresolved}
        for todo in plan.todos:
            if todo.child and todo.label not in run.active and todo.label not in unresolved:
                await run.launch(todo)
        for todo in plan.ready():
            mine = todo._doc.get("delegation") or todo.label in plan._inline
            if not mine or todo.key in run.refusals or todo.label in unresolved:
                continue
            refusal = await run.launch(todo)
            if refusal is not None and refusal.kind == "admission":
                run.refusals.pop(todo.key, None)
                break
        if not run.active:
            return
        await run.settle()


_install()
