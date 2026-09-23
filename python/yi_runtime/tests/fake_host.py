"""A stand-in for the host's `plan.op`, `rlm.wait`, `rlm.result` and `fetch` requests.

It keeps the rules the `yi` library leans on (request ids replay, one open plan,
edges gate `start`, a contract freezes at `start` and decides at `done`, the step
table, the admission count, the engine starting admitted delegated todos after every
op and finishing one whose child ended, the retry cap, `wait` with a cursor) and none
of the engine's depth; the T2 journeys in `kernel_data_surface.rs` run the real one.
"""
from __future__ import annotations

import asyncio
import collections
import copy
import hashlib
import json

import rlm


# `TodoLabel::new` (`ids.rs`): 80 characters, and a newline is refused.
LABEL_MAX = 80
# The engine's step table (`table.rs`), for the ops a shape issues on its own.
LEGAL = {
    "done": ("running",),
    "decompose": ("running",),
    "fail": ("running",),
    "block": ("pending", "running"),
    "unblock": ("blocked",),
    "retry": ("failed",),
    "drop": ("pending", "blocked"),
}


def refusal(kind: str, message: str, code: str = "refused", **extra) -> dict:
    return {"ok": False, "refusal": {"code": code, "kind": kind, "message": message, **extra}}


class FakeHost:
    def __init__(self) -> None:
        self.plans: dict[str, dict] = {}
        self.journal: list[tuple[str, dict]] = []
        self.replies: dict[str, tuple[str, dict]] = {}
        self.blobs: dict[str, str] = {}
        self.children: dict[str, str] = {}
        self.results: dict[str, dict] = {}
        self.verdicts: dict[str, str] = {}
        self.notices: list[str] = []
        self.files: dict[str, str] = {}
        self.spawns = 0
        self.slots: int | None = None
        self.retry_cap = 8
        self.starts: list[str] = []
        self.refused: dict[tuple[str, str, int], str] = {}
        self.finished: set[tuple[str, str, int]] = set()
        # Waits a todo's finish is still in flight for, and the finishes that left a todo running.
        self.lag: dict[str, int] = {}
        # Seconds the engine takes to settle an ended child, on its own clock: a wait then
        # blocks as the host's does, and a settle moves nothing a waiter sees.
        self.settles_after: float | None = None
        self.ended_at: dict[tuple[str, str, int], float] = {}
        self.left: dict[tuple[str, str, int], str] = {}
        # The engine's own finish ops, apart from the journal of what the library sent.
        self.engine_ops: list[tuple[str, dict]] = []
        # Every op the library sent, refused or not.
        self.sent: list[str] = []
        # Labels whose start the engine is refused, with the reason the host gives.
        self.unstartable: dict[str, str] = {}
        self.requests: collections.Counter = collections.Counter()
        self.waits: list[tuple[int | None, list[str]]] = []
        self.changes: list[tuple[str, str]] = []
        rlm.host_request = self.request

    def ops(self) -> list[str]:
        return [op for op, _ in self.journal]

    async def request(self, kind: str, payload: dict | None = None) -> dict:
        payload = payload or {}
        self.requests[kind] += 1
        if kind == "plan.op":
            if self.settles_after is not None:
                self.finish()
            return self.plan_op(payload)
        if kind == "rlm.wait" and self.settles_after is not None:
            return await self.quiet_wait(payload)
        if kind == "rlm.wait":
            await asyncio.sleep(0.01)
            # The engine takes a child's end before a waiter reads it: a set-up that marks a
            # child finished at declaration is finished when the run first waits, not sooner.
            self.finish()
            # Each caller's cursor is its own place in the change log; nobody drains it.
            latest = dict(self.changes)
            self.changes += [item for item in self.children.items() if latest.get(item[0]) != item[1]]
            changed = sorted({name for name, _ in self.changes[payload.get("cursor") or 0 :]})
            self.waits.append((payload.get("cursor"), changed))
            return {"cursor": len(self.changes), "changed": changed, "states": dict(self.children), "notes": {}}
        if kind == "rlm.result":
            answer = self.results[payload["target"]]
            if isinstance(answer, Exception):
                raise answer
            return answer
        if kind == "rlm.interrupt":
            self.children[payload["target"]] = "failed"
            return {}
        if kind == "fetch":
            url = payload["url"]
            text = self.files.get(url, self.blobs.get("sha256:" + url.rsplit("/", 1)[-1]))
            if text is None:
                raise RuntimeError(f"{url}: not found")
            return {"text": text}
        raise AssertionError(f"unexpected host request {kind}")

    async def quiet_wait(self, payload: dict) -> dict:
        """`mailbox.rs` wait: it answers once a child moved past the cursor, or at the timeout."""
        loop = asyncio.get_running_loop()
        ends, cursor = loop.time() + payload["timeout_ms"] / 1000, payload.get("cursor") or 0
        while True:
            latest = dict(self.changes)
            self.changes += [item for item in self.children.items() if latest.get(item[0]) != item[1]]
            if len(self.changes) > cursor or loop.time() >= ends:
                changed = sorted({name for name, _ in self.changes[cursor:]})
                self.waits.append((payload.get("cursor"), changed))
                return {"cursor": len(self.changes), "changed": changed, "states": dict(self.children), "notes": {}}
            await asyncio.sleep(0.01)

    def plan_op(self, payload: dict) -> dict:
        op, args, request_id = payload["op"], payload.get("args") or {}, payload["request_id"]
        self.sent.append(op)
        identity = json.dumps([op, args], sort_keys=True)
        if request_id in self.replies:
            seen, reply = self.replies[request_id]
            if seen != identity:
                return refusal("request_id_reused", "the id was used with other args", "request_id_reused")
            return copy.deepcopy(reply)
        for blob in payload.get("artifacts") or []:
            digest = "sha256:" + hashlib.sha256(blob["text"].encode()).hexdigest()
            self.blobs[digest] = blob["text"]
        reply = self.apply(op, args, payload.get("plan"), payload.get("expected_revision"))
        if reply["ok"] and op not in ("view", "repair"):
            self.journal.append((op, copy.deepcopy(args)))
        if reply["ok"] and op != "view":
            self.dispatch()
            reply["revision"] = reply["plan"]["touched"]
        if reply["ok"] and op not in ("view", "repair"):
            self.replies[request_id] = (identity, copy.deepcopy(reply))
        return copy.deepcopy(reply)

    def dispatch(self) -> None:
        """`schedule.rs`: after an op the engine starts each ready delegated todo admission lets
        through; a held one is never tried, and a refused one is tried once per attempt."""
        for plan in list(self.plans.values()):
            if plan["state"] != "active":
                continue
            for label in self.view(plan)["plan"]["ready"]:
                todo = next(item for item in plan["todos"] if item["label"] == label)
                key = (plan["plan"], label, todo["attempt"])
                if not todo.get("delegation") or key in self.refused or self.held(plan, todo) is not None:
                    continue
                refused = self.step("start", {"label": label}, plan, todo)
                if refused is None:
                    plan["touched"] += 1
                    self.journal.append(("start", {"label": label}))
                else:
                    self.refused[key] = refused["refusal"]["message"]
            self.view(plan)

    def finish(self) -> None:
        """`finish.rs`: a delegated todo whose child ended is the engine's: submitted and done,
        failed on a red verdict or a failed child, left running on any other verdict."""
        for plan in list(self.plans.values()):
            for todo in plan["todos"]:
                child, key = todo.get("by"), (plan["plan"], todo["label"], todo["attempt"])
                ended = self.children.get(child) in ("finished", "failed")
                if todo["state"] != "running" or not todo.get("delegation") or not ended or key in self.finished:
                    continue
                if self.lag.get(todo["label"], 0) > 0:
                    self.lag[todo["label"]] -= 1
                    continue
                if self.settles_after is not None:
                    now = asyncio.get_running_loop().time()
                    if now - self.ended_at.setdefault(key, now) < self.settles_after:
                        continue
                self.finished.add(key)
                if self.children[child] == "failed":
                    self.engine("fail", {"label": todo["label"], "cause": f"child {child} failed"}, plan, todo)
                    continue
                answer = self.results.get(child)
                said = answer.get("json", answer.get("text", "")) if isinstance(answer, dict) else ""
                text = said if isinstance(said, str) else json.dumps(said, sort_keys=True, separators=(",", ":"))
                digest = hashlib.sha256(text.encode()).hexdigest()
                self.blobs["sha256:" + digest] = text
                if not todo.get("submitted"):
                    self.engine("submit", {"label": todo["label"], "output": f"plan://{plan['plan']}/artifacts/{digest}"}, plan, todo)
                refused = self.engine("done", {"label": todo["label"], "output": todo["submitted"]}, plan, todo)
                outcome = refused and refused["refusal"]["verdict"]["outcome"]
                if outcome == "fail":
                    self.engine("fail", {"label": todo["label"], "cause": "contract refused: the verdict was fail"}, plan, todo)
                elif outcome:
                    self.left[key] = f"its verdict was {outcome}"
        self.dispatch()

    def engine(self, op: str, args: dict, plan: dict, todo: dict) -> dict | None:
        """One op as the engine, kept apart so a test reads what the library itself sent."""
        refused = self.step(op, args, plan, todo)
        if refused is None:
            plan["touched"] += 1
            self.engine_ops.append((op, copy.deepcopy(args)))
        return refused

    def held(self, plan: dict, todo: dict) -> int | None:
        """`table.rs` admit: None when a ready todo may start, else the free slots it waits on."""
        if not todo.get("delegation") or self.slots is None:
            return None
        ready = self.view(plan)["plan"]["ready"]
        by = {item["label"]: item for item in plan["todos"]}
        queue = [label for label in ready if by[label].get("delegation")]
        free = self.slots - sum(1 for item in plan["todos"] if item["state"] == "running" and item.get("delegation"))
        return None if queue.index(todo["label"]) < free else max(free, 0)

    def view(self, plan: dict, notices: list[str] | None = None) -> dict:
        cleared = {todo["label"] for todo in plan["todos"] if todo["state"] in ("done", "abandoned")}
        plan["ready"] = [
            todo["label"]
            for todo in plan["todos"]
            if todo["state"] == "pending" and set(todo.get("after", [])) <= cleared
        ]
        by = {todo["label"]: todo for todo in plan["todos"]}
        queue = [label for label in plan["ready"] if by[label].get("delegation")]
        running = sum(1 for todo in plan["todos"] if todo["state"] == "running" and todo.get("delegation"))
        held = [] if self.slots is None else queue[max(self.slots - running, 0) :]
        # `schedule.rs::Standing`: by label for the library, and the prose `{:?}` quotes for a reader.
        unstarted = {label: text for (at, label, attempt), text in self.refused.items() if at == plan["plan"] and label in queue and by[label]["attempt"] == attempt}
        left = {label: text for (at, label, attempt), text in self.left.items() if at == plan["plan"] and by[label]["attempt"] == attempt and by[label]["state"] == "running"}
        said = [f"the engine could not start {json.dumps(label)}: {text}" for label, text in unstarted.items()]
        said += [f"the engine left {json.dumps(label)} running: {text}" for label, text in left.items()]
        reply = {"ok": True, "revision": plan["touched"], "text": "", "plan": plan, "held": held, "unstarted": unstarted, "left": left}
        return {**reply, "notices": said if notices is None else notices}

    def apply(self, op: str, args: dict, plan_id: str | None, expected: int | None) -> dict:
        if op == "init":
            if any(plan["state"] == "active" for plan in self.plans.values()):
                return refusal("plan_exists", "a plan is already open")
            plan_id = args["goal"].lower().replace(" ", "-")
            self.plans[plan_id] = {"plan": plan_id, "goal": args["goal"], "touched": 1, "state": "active", "todos": []}
            return self.view(self.plans[plan_id])
        plan = self.plans.get(plan_id or "")
        if plan is None:
            return refusal("no_plan", f"no plan {plan_id}")
        if op == "view":
            return self.view(plan)
        if op == "repair":
            return self.view(plan, list(self.notices))
        if expected is not None and expected != plan["touched"]:
            return refusal("stale_revision", "the plan moved", "stale_revision")
        todo = next((todo for todo in plan["todos"] if todo["label"] == args.get("label")), None)
        refused = self.step(op, args, plan, todo)
        if refused is not None:
            return refused
        plan["touched"] += 1
        return self.view(plan)

    def step(self, op: str, args: dict, plan: dict, todo: dict | None) -> dict | None:
        if op in LEGAL and todo["state"] not in LEGAL[op]:
            return refusal("illegal_step", f"{op} is illegal for {todo['label']} while {todo['state']}")
        for spec in args.get("todos") or []:
            if len(spec["label"]) > LABEL_MAX or "\n" in spec["label"]:
                return refusal("label", f"{spec['label'][:40]!r}… is not a legal todo label")
        if op == "program":
            if args["source_ref"]["digest"] not in self.blobs:
                return refusal("program", "the source is not in the store")
        elif op == "append":
            plan["todos"] += [{**spec, "state": "pending", "attempt": 1} for spec in args["todos"]]
        elif op == "decompose":
            todo["subplan"] = f"{plan['plan']}.{todo['label']}"
            self.plans[todo["subplan"]] = {
                "plan": todo["subplan"],
                "goal": todo["label"],
                "touched": 1,
                "state": "active",
                "todos": [{**spec, "state": "pending", "attempt": 1} for spec in args["todos"]],
            }
        elif op == "start":
            self.starts.append(todo["label"])
            ready = self.view(plan)["plan"]["ready"]
            if todo["label"] not in ready:
                return refusal("illegal_step", f"start is illegal for {todo['label']}")
            if todo["label"] in self.unstartable:
                return refusal("contract", self.unstartable[todo["label"]])
            free = self.held(plan, todo)
            if free is not None:
                return refusal("admission", f"{todo['label']} waits for a slot; {free} free")
            for item in (todo.get("contract") or {}).get("items", []):
                for ref in next(iter(item["decider"].values())).values():
                    if isinstance(ref, dict) and ref["digest"] not in self.blobs:
                        return refusal("contract", f"criterion {ref['digest']} is not in the store")
            todo["state"], todo["by"] = "running", "main"
            if todo.get("delegation"):
                self.spawns += 1
                todo["by"] = plan["plan"] + "/" + todo["label"].split(":")[0]
                self.children.setdefault(todo["by"], "running")
        elif op == "submit":
            todo["submitted"] = args["output"]
        elif op == "done":
            outcome = self.verdicts.get(todo["label"], "pass")
            if isinstance(outcome, list):
                outcome = outcome.pop(0) if len(outcome) > 1 else outcome[0]
            if todo.get("contract"):
                if outcome != "pass":
                    return refusal("refused", "done refused", verdict={"outcome": outcome, "items": []})
                todo["resolution"] = "verified_done"
            todo["state"], todo["output"] = "done", args.get("output")
        elif op == "block":
            todo["state"], todo["on"], todo["note"] = "blocked", args["on"], args["note"]
        elif op == "fail":
            todo["state"], todo["cause"] = "failed", args["cause"]
        elif op == "retry":
            if todo["attempt"] > self.retry_cap:
                return refusal("retries_exhausted", f"{todo['label']} spent its {self.retry_cap} retries")
            todo["state"], todo["attempt"] = "pending", todo["attempt"] + 1
        elif op == "drop":
            todo["state"] = "abandoned"
        elif op == "add_edge":
            waits = next(item for item in plan["todos"] if item["label"] == args["todo"])
            waits["after"] = [*waits.get("after", []), args["after"]]
        else:
            raise AssertionError(f"the fake host has no op {op}")
        return None
