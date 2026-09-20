"""A stand-in for the host's `plan.op`, `rlm.wait`, `rlm.result` and `fetch` requests.

It keeps the rules the `yi` library leans on (request ids replay, one open plan,
edges gate `start`, a contract freezes at `start` and decides at `done`) and none
of the engine's depth; the T2 journeys in `kernel_data_surface.rs` run the real one.
"""
from __future__ import annotations

import asyncio
import copy
import hashlib
import json

import rlm


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
        rlm.host_request = self.request

    def ops(self) -> list[str]:
        return [op for op, _ in self.journal]

    async def request(self, kind: str, payload: dict | None = None) -> dict:
        payload = payload or {}
        if kind == "plan.op":
            return self.plan_op(payload)
        if kind == "rlm.wait":
            await asyncio.sleep(0.01)
            return {"cursor": 1, "changed": [], "states": dict(self.children), "notes": {}}
        if kind == "rlm.result":
            return self.results[payload["target"]]
        if kind == "rlm.interrupt":
            self.children[payload["target"]] = "failed"
            return {}
        if kind == "fetch":
            return {"text": self.files[payload["url"]]}
        raise AssertionError(f"unexpected host request {kind}")

    def plan_op(self, payload: dict) -> dict:
        op, args, request_id = payload["op"], payload.get("args") or {}, payload["request_id"]
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
            self.replies[request_id] = (identity, copy.deepcopy(reply))
        return copy.deepcopy(reply)

    def view(self, plan: dict, notices: list[str] | None = None) -> dict:
        cleared = {todo["label"] for todo in plan["todos"] if todo["state"] in ("done", "abandoned")}
        plan["ready"] = [
            todo["label"]
            for todo in plan["todos"]
            if todo["state"] == "pending" and set(todo.get("after", [])) <= cleared
        ]
        return {"ok": True, "revision": plan["touched"], "text": "", "plan": plan, "notices": notices or []}

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
        if op == "program":
            if args["source_ref"]["digest"] not in self.blobs:
                return refusal("program", "the source is not in the store")
        elif op == "append":
            plan["todos"] += [{**spec, "state": "pending", "attempt": 1} for spec in args["todos"]]
        elif op == "start":
            if todo["label"] not in self.view(plan)["plan"]["ready"]:
                return refusal("illegal_step", f"start is illegal for {todo['label']}")
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
            if todo["state"] != "running":
                return refusal("illegal_step", f"done is illegal for {todo['label']}")
            if todo.get("contract"):
                if self.verdicts.get(todo["label"], "pass") != "pass":
                    return refusal("refused", "done refused", verdict={"outcome": "fail", "items": []})
                todo["resolution"] = "verified_done"
            todo["state"], todo["output"] = "done", args.get("output")
        elif op == "fail":
            todo["state"], todo["cause"] = "failed", args["cause"]
        elif op == "retry":
            todo["state"], todo["attempt"] = "pending", todo["attempt"] + 1
        elif op == "drop":
            todo["state"] = "abandoned"
        else:
            raise AssertionError(f"the fake host has no op {op}")
        return None
