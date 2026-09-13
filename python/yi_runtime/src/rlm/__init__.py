"""Yi's kernel-side runtime shim (module name `rlm`)."""

from __future__ import annotations

import asyncio
import json
import os
import sys
import pathlib
import time
import re
import types
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .harness import HarnessEntry, HarnessScope, HarnessState, RefinementEvent, get_harness_state

try:
    from ipykernel.comm import Comm
except Exception:  # pragma: no cover - depends on ipykernel version
    Comm = None  # type: ignore[assignment]

try:
    from IPython import get_ipython
except Exception:  # pragma: no cover - only available in kernels
    get_ipython = None  # type: ignore[assignment]

HOST_COMM_TARGET = "host.request"


def _check_schema(schema: dict[str, Any] | None) -> None:
    if schema is not None and not isinstance(schema, dict):
        raise TypeError(f"schema must be a dict or None, got {type(schema).__name__}")


@dataclass(frozen=True)
class RLMSpawnHandle:
    rlm_child_id: str
    name: str
    session_dir: Path
    model: str
    next: str = ""

    def __repr__(self) -> str:
        head = (
            f"RLMSpawnHandle(name={self.name!r}, model={self.model!r}, "
            f"session_dir={str(self.session_dir)!r})"
        )
        return f"{head}\n{self.next}" if self.next else head

    async def result(
        self,
        *,
        schema: dict[str, Any] | None = None,
        # Under the kernel cell's 600 s wall clock (D176): a 900 s wait could never
        # elapse, the cell was aborted first and the child's answer never came back.
        timeout: float = 540.0,
    ) -> dict[str, Any]:
        """Wait for this child to finish and return its answer as data.

        The reply carries ``text`` and, when the child answered with JSON,
        ``json``. A ``schema`` is checked host-side and a mismatch raises,
        so a malformed result never reaches the parent's transcript as if
        it had passed. The wait is ``rlm.wait`` with this call's own cursor,
        so the host blocks instead of the kernel polling, and no other waiter
        can steal this child's update. The deadline is wall clock, and a
        child no longer registered with the parent (``delete_subagent``
        reaps it) raises immediately instead of waiting to the deadline.
        """
        _check_schema(schema)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        cursor: int | None = None
        while True:
            remaining = deadline - loop.time()
            if remaining <= 0:
                break
            reply = await wait(timeout=remaining, cursor=cursor)
            states = reply.get("states")
            if not isinstance(states, dict):
                raise RuntimeError("rlm.wait returned an invalid state map")
            next_cursor = reply.get("cursor")
            if isinstance(next_cursor, int):
                cursor = next_cursor
            state = states.get(self.name)
            if state is None:
                raise RuntimeError(
                    f"child {self.name} ({self.rlm_child_id}) is no longer registered with "
                    "the parent; rlm.delete_subagent reaps a child and its answer with it"
                )
            if state in ("finished", "failed", "needs_you"):
                try:
                    return await result(self.rlm_child_id, schema=schema)
                except RuntimeError as error:
                    # needs_you also names a running child blocked on the user; only a
                    # child that ended asking its parent has an answer to collect.
                    if state == "needs_you" and "still running" in str(error):
                        continue
                    raise
        raise TimeoutError(f"child {self.name} did not finish within {timeout}s")

    async def send(self, message: str, followup: bool = False) -> dict[str, Any]:
        return await send(self.name, message, followup)


@dataclass(frozen=True)
class RLMModel:
    provider: str
    id: str
    name: str
    selector: str


@dataclass(frozen=True)
class RLMSubagent:
    rlm_child_id: str
    active_session_id: str | None
    session_id: str | None
    session_name: str
    session_dir: Path
    status: str

    @property
    def name(self) -> str:
        return self.session_name


def _install_control_comm_handlers() -> None:
    """Let comm replies arrive on the control channel during an execute_request."""
    if get_ipython is None:
        return
    shell = get_ipython()
    kernel = getattr(shell, "kernel", None)
    comm_manager = getattr(kernel, "comm_manager", None)
    control_handlers = getattr(kernel, "control_handlers", None)
    if comm_manager is None or not isinstance(control_handlers, dict):
        return
    control_handlers.setdefault("comm_msg", comm_manager.comm_msg)
    control_handlers.setdefault("comm_close", comm_manager.comm_close)


def _spawn_handle_from_payload(payload: Any) -> RLMSpawnHandle:
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.run returned an invalid spawn handle")
    child_id = payload.get("rlm_child_id")
    name = payload.get("name")
    session_dir = payload.get("session_dir")
    model = payload.get("model")
    if not all(isinstance(value, str) and value for value in (child_id, name, session_dir, model)):
        raise RuntimeError("rlm.run returned an invalid spawn handle")
    following = payload.get("next")
    return RLMSpawnHandle(
        rlm_child_id=child_id,
        name=name,
        session_dir=Path(session_dir),
        model=model,
        next=following if isinstance(following, str) else "",
    )


async def host_request(request_type: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
    """Send a typed request to the Yi host and await its reply.

    This is the kernel side of the generic host bridge: Python skills call
    ``await host_request("<type>", {...})`` and the TypeScript host dispatches
    on the type. Raises RuntimeError when the host reports an error or when no
    handler for the type is registered in this session.
    """
    if not isinstance(request_type, str) or not request_type:
        raise TypeError("request_type must be a non-empty str")
    if payload is not None and not isinstance(payload, dict):
        raise TypeError(f"payload must be a dict or None, got {type(payload).__name__}")
    if Comm is None:
        raise RuntimeError("Jupyter comm support is unavailable in this kernel")
    _install_control_comm_handlers()

    loop = asyncio.get_running_loop()
    future: asyncio.Future[dict[str, Any]] = loop.create_future()
    comm = Comm(target_name=HOST_COMM_TARGET, primary=False)

    def _on_msg(msg: dict[str, Any]) -> None:
        content = msg.get("content", {})
        reply = content.get("data", {}) if isinstance(content, dict) else {}
        if not isinstance(reply, dict):
            return

        status = reply.get("status")
        if status == "ok":
            def _resolve_result() -> None:
                if not future.done():
                    future.set_result({k: v for k, v in reply.items() if k != "status"})
                    comm.close()

            loop.call_soon_threadsafe(_resolve_result)
            return
        if status == "error":
            message = reply.get("error") or f"host request {request_type} failed"
            def _resolve_error() -> None:
                if not future.done():
                    future.set_exception(RuntimeError(str(message)))
                    comm.close()

            loop.call_soon_threadsafe(_resolve_error)
            return

        unexpected = f"host request {request_type} returned unexpected status: {status!r}"
        def _resolve_unexpected() -> None:
            if not future.done():
                future.set_exception(RuntimeError(unexpected))
                comm.close()

        loop.call_soon_threadsafe(_resolve_unexpected)

    comm.on_msg(_on_msg)
    # request_type goes last so a payload "type" key cannot reroute the request.
    comm.open(data={**(payload or {}), "type": request_type})
    try:
        return await future
    finally:
        if not future.done():
            future.cancel()
        comm.close()


CONTEXT_VALUE_CAP = 4096


def _resolve_context(kwargs: dict[str, Any]) -> dict[str, Any]:
    """Serialize the named kernel variables here; the host cannot read them.

    A host-side read would be an execute_request queued behind the very cell
    awaiting this spawn, so the scope is resolved in the namespace that owns
    it. A name that is not bound raises before any host round trip.
    """
    keys = kwargs.pop("context_keys", None)
    if keys is None:
        return kwargs
    if isinstance(keys, str) or not all(isinstance(key, str) for key in keys):
        raise TypeError("context_keys must be a list of variable names")
    namespace = get_ipython().user_ns if get_ipython is not None else {}
    context: dict[str, str] = {}
    for key in keys:
        if key not in namespace:
            raise KeyError(f"context_keys names {key!r}, which is not bound in this kernel")
        try:
            text = json.dumps(namespace[key], default=repr)
        except Exception:
            text = repr(namespace[key])
        context[key] = text[:CONTEXT_VALUE_CAP]
    kwargs["context"] = context
    return kwargs


async def run(prompt: str, **kwargs: Any) -> RLMSpawnHandle:
    """Spawn a recursive Yi child and return once its task is admitted.

    ``model`` selects a child with an exact ``provider/model`` selector.
    ``thinking`` sets the child reasoning level (e.g. 'off', 'low', 'medium', 'high');
    defaults to the parent level; levels invalid for the resolved model fail the spawn.
    ``fork`` seeds the child with this session's history: 'none' (default), 'all'
    (inherits the parent model, so ``model``/``thinking`` are refused with it), or a
    positive turn count for the last N turns.
    ``isolation='worktree'`` gives the child its own checkout; hand it back with
    ``merge_worktree`` or ``discard_worktree``.
    ``deny_write`` (and ``deny_read``) are lists of paths the child may not touch —
    the wall that keeps an implementer out of the standard it is measured against.
    ``deny_url`` is the same wall in URL space: a list of literal prefixes the
    child's ``fetch`` refuses, so ``["kernel://"]`` walls a whole scheme.
    ``context_keys`` is the child's whole view of this kernel: those variables are
    serialized into its brief and nothing else of this namespace reaches it.
    ``check`` makes it a protocol child — it owes a ``{"value": …, "discoveries":
    […]}`` answer, and ``result`` withholds that answer while the check is red.
    """
    if not isinstance(prompt, str):
        raise TypeError(f"prompt must be str, got {type(prompt).__name__}")
    kwargs = _resolve_context(kwargs)
    payload = await host_request("rlm.run", {"prompt": prompt, "kwargs": kwargs})
    return _spawn_handle_from_payload(payload)


def _model_from_payload(payload: Any) -> RLMModel:
    if not isinstance(payload, dict):
        raise RuntimeError("rlm.find_models returned an invalid model entry")
    provider = payload.get("provider")
    model_id = payload.get("id")
    name = payload.get("name")
    selector = payload.get("selector")
    if not all(isinstance(value, str) and value for value in (provider, model_id, name, selector)):
        raise RuntimeError("rlm.find_models returned an invalid model entry")
    return RLMModel(provider=provider, id=model_id, name=name, selector=selector)


async def find_models(query: str = "", limit: int = 8) -> list[RLMModel]:
    """Search a bounded list of models backed by active user credentials."""
    if not isinstance(query, str):
        raise TypeError(f"query must be str, got {type(query).__name__}")
    if not isinstance(limit, int):
        raise TypeError(f"limit must be int, got {type(limit).__name__}")
    payload = await host_request("rlm.find_models", {"query": query, "limit": limit})
    models = payload.get("models")
    if not isinstance(models, list):
        raise RuntimeError("rlm.find_models returned an invalid models list")
    return [_model_from_payload(model) for model in models]


def _subagent_from_payload(payload: Any, operation: str = "rlm.list_subagents") -> RLMSubagent:
    if not isinstance(payload, dict):
        raise RuntimeError(f"{operation} returned an invalid subagent entry")
    child_id = payload.get("rlm_child_id")
    active_session_id = payload.get("active_session_id")
    session_id = payload.get("session_id")
    session_name = payload.get("session_name")
    session_dir = payload.get("session_dir")
    status = payload.get("status")
    if not isinstance(child_id, str) or not child_id:
        raise RuntimeError(f"{operation} entry is missing rlm_child_id")
    if active_session_id is not None and not isinstance(active_session_id, str):
        raise RuntimeError(f"{operation} entry has invalid active_session_id")
    if session_id is not None and not isinstance(session_id, str):
        raise RuntimeError(f"{operation} entry has invalid session_id")
    if not isinstance(session_name, str) or not session_name:
        raise RuntimeError(f"{operation} entry is missing session_name")
    if not isinstance(session_dir, str) or not session_dir:
        raise RuntimeError(f"{operation} entry is missing session_dir")
    if status not in {"running", "completed", "error"}:
        raise RuntimeError(f"{operation} entry has invalid status")
    return RLMSubagent(
        rlm_child_id=child_id,
        active_session_id=active_session_id,
        session_id=session_id,
        session_name=session_name,
        session_dir=Path(session_dir),
        status=status,
    )


async def list_subagents() -> list[RLMSubagent]:
    """List direct RLM children retained by the current parent session."""
    payload = await host_request("rlm.list_subagents")
    entries = payload.get("subagents")
    if not isinstance(entries, list):
        raise RuntimeError("rlm.list_subagents returned an invalid subagents registry")
    return [_subagent_from_payload(entry) for entry in entries]


async def delete_subagent(target: str | RLMSubagent) -> RLMSubagent:
    """Delete one running or retained direct child from the current parent session."""
    if isinstance(target, RLMSubagent):
        selector = target.rlm_child_id
    elif isinstance(target, str):
        selector = target.strip()
        if not selector:
            raise ValueError("target must not be empty")
    else:
        raise TypeError(f"target must be str or RLMSubagent, got {type(target).__name__}")
    payload = await host_request("rlm.delete_subagent", {"target": selector})
    return _subagent_from_payload(payload.get("subagent"), "rlm.delete_subagent")


async def send(target: "str | RLMSubagent", message: str, followup: bool = False) -> dict[str, Any]:
    """Send one agent message; ``followup=True`` also starts the target's turn.

    ``target`` is an agent name, ``"parent"``, or ``"all"``. A broadcast returns
    one receipt per target rather than failing whole on the first bad one.
    """
    if not isinstance(message, str) or not message:
        raise ValueError("message must be a non-empty str")
    selector = target if isinstance(target, str) else target.session_name
    return await host_request(
        "agent_message.send",
        {"target": selector, "message": message, "followup": bool(followup)},
    )


async def followup(target: "str | RLMSubagent", message: str) -> dict[str, Any]:
    """Send and start the target's turn if it is idle (delivered at a boundary if not)."""
    return await send(target, message, followup=True)


async def status(name: str | None = None) -> list[dict[str, Any]]:
    """Every child's state as its own records show it (D165).

    Each entry is ``{name, state, note, tools, tokens, idle_s, worktree}`` with ``state``
    one of ``running``, ``finished``, ``failed``, ``needs_you`` (it ended on ``ask_user``
    or blocked a todo on you: answer with ``send(name, text, followup=True)``) and
    ``stuck`` (a repeat break, a length re-drive at rung two or more, a let-go
    intercept, or five idle minutes; ``note`` names which). ``name`` keeps one.
    """
    payload = await host_request("rlm.status", {})
    members = payload.get("members")
    if not isinstance(members, list):
        raise RuntimeError("rlm.status returned an invalid member list")
    if name is not None:
        members = [member for member in members if member.get("name") == name]
    return members


async def list_agents() -> list[dict[str, Any]]:
    """The family this agent can address by name."""
    payload = await host_request("agent_message.list_agents", {})
    agents = payload.get("agents")
    if not isinstance(agents, list):
        raise RuntimeError("agent_message.list_agents returned an invalid roster")
    return agents


async def wait(timeout: float = 300.0, cursor: int | None = None) -> dict[str, Any]:
    """Block until a child reports or finishes; returns what moved since ``cursor``.

    The reply carries ``cursor`` (pass it back on the next call and no other
    waiter can steal your updates), ``changed`` and its one-release alias
    ``updated`` (the names that moved), ``states`` (every registered child by
    name: ``running``, ``finished``, ``failed``, ``needs_you`` or ``stuck``)
    and ``notes``. Called with no cursor you see the family as it stands now,
    so a child that finished before you called is still terminal in ``states``.
    The host clamps the timeout and says so in the reply (``clamped``), so a
    caller is never silently given a different one.
    """
    payload: dict[str, Any] = {"timeout_ms": int(timeout * 1000)}
    if cursor is not None:
        payload["cursor"] = cursor
    return await host_request("rlm.wait", payload)


async def plan_op(
    op: str,
    args: dict[str, Any] | None = None,
    *,
    plan: str | None = None,
    request_id: str | None = None,
    expected_revision: int | None = None,
) -> dict[str, Any]:
    """Run one op against the host's plan engine and return its reply.

    The same parser and the same engine the JSON plan tool uses; the host
    fixes the actor from the registry this kernel is wired to, so a child
    kernel gets its parent's plan read-only whatever it asks for. ``plan``
    names a plan other than the session's and ``expected_revision`` refuses
    the op if the plan moved under you. Pass your own ``request_id`` to retry
    as the same request; without one each call is minted a new id and is a
    new request. Read ``ok`` in the reply: a refusal is data, not an
    exception.

        reply = await plan_op("view", {"full": True})
    """
    payload: dict[str, Any] = {
        "request_id": request_id or str(uuid.uuid4()),
        "plan": plan,
        "expected_revision": expected_revision,
        "op": op,
        "args": args,
    }
    return await host_request("plan.op", {k: v for k, v in payload.items() if v is not None})


async def interrupt(target: "str | RLMSubagent") -> dict[str, Any]:
    """End a child's run and keep its record (``delete_subagent`` reaps instead)."""
    return await host_request("rlm.interrupt", {"target": _worktree_target(target)})


async def result(
    target: "str | RLMSubagent", *, schema: dict[str, Any] | None = None
) -> dict[str, Any]:
    """A finished child's answer as data, checked against ``schema`` host-side."""
    _check_schema(schema)
    payload: dict[str, Any] = {"target": _worktree_target(target)}
    if schema is not None:
        payload["schema"] = schema
    return await host_request("rlm.result", payload)


def _worktree_target(target: "str | RLMSubagent") -> str:
    if isinstance(target, RLMSubagent):
        return target.rlm_child_id
    if isinstance(target, str):
        selector = target.strip()
        if not selector:
            raise ValueError("target must not be empty")
        return selector
    raise TypeError(f"target must be str or RLMSubagent, got {type(target).__name__}")


async def merge_worktree(target: "str | RLMSubagent") -> dict[str, Any]:
    """Commit an isolated child's work on its branch and merge it into this checkout.

    The child must have finished. The worktree is removed once merged, which is
    also what releases the child for ``delete_subagent``.
    """
    return await host_request("rlm.merge_worktree", {"target": _worktree_target(target)})


async def discard_worktree(target: "str | RLMSubagent") -> dict[str, Any]:
    """Throw an isolated child's worktree and branch away without merging."""
    return await host_request("rlm.discard_worktree", {"target": _worktree_target(target)})


_KERNEL_SCHEME = "kernel://"
_OWNER_AGENT = "main"


def _kernel_local_name(url: str) -> str | None:
    """The variable this URL names in this kernel's own namespace, or None.

    Only the plan owner's kernel resolves locally: a child kernel (its session
    dir is a ``sub-`` directory) forwards every ``kernel://`` URL to the host,
    because an elided owner means the plan owner's namespace, not its own.
    """
    if not url.startswith(_KERNEL_SCHEME):
        return None
    session_dir = os.environ.get("RLM_SESSION_DIR", "")
    basename = os.path.basename(os.path.normpath(session_dir)) if session_dir else ""
    if basename.startswith("sub-"):
        return None
    rest = url[len(_KERNEL_SCHEME):]
    owner, slash, variable = rest.partition("/")
    if not slash:
        owner, variable = _OWNER_AGENT, owner
    if owner != _OWNER_AGENT or not variable.isidentifier():
        return None
    return variable


async def fetch(url: str, *, as_text: bool = False) -> Any:
    """Read one addressable URL; a URL names a noun, so a fetch never writes.

    ``kernel://<var>`` and ``kernel://main/<var>`` in the plan owner's kernel
    read this namespace directly and return the live object, with no host
    round trip. Every other URL — ``local://``, ``plan://``, ``history://``,
    ``checkpoint://``, ``mcp://``, another agent's ``kernel://`` — is resolved
    by the host and returns text. Batch reads with ``asyncio.gather``.
    """
    if not isinstance(url, str) or not url:
        raise TypeError("url must be a non-empty str")
    name = _kernel_local_name(url)
    if name is not None:
        shell = get_ipython() if get_ipython is not None else None
        if shell is None:
            raise RuntimeError(f"{url} reads this kernel's namespace, but no IPython shell is active")
        namespace = shell.user_ns
        if name not in namespace:
            raise KeyError(f"{url} names {name!r}, which is not bound in this kernel")
        return namespace[name]
    # D164: another member's kernel:// returns the object itself (its kernel dills it into
    # the family dir); text on request, or when the host has no family dir to dill into.
    want_object = url.startswith("kernel://") and not as_text
    reply = await host_request("fetch", {"url": url, "object": want_object})
    path = reply.get("path")
    if want_object and isinstance(path, str):
        with open(path, "rb") as handle:
            return _serializer().load(handle)
    text = reply.get("text")
    if not isinstance(text, str):
        raise RuntimeError(f"fetch of {url} returned no text")
    return text


def _serializer() -> Any:
    try:
        import dill

        return dill
    except ImportError:
        import pickle

        return pickle


FAMILY_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")


def _family_dir() -> pathlib.Path:
    raw = os.environ.get("RLM_FAMILY_DIR", "")
    if not raw:
        raise RuntimeError("no family directory: RLM_FAMILY_DIR is unset in this kernel")
    return pathlib.Path(raw)


def _member_name() -> str:
    session_dir = os.environ.get("RLM_SESSION_DIR", "")
    base = os.path.basename(session_dir.rstrip("/"))
    return base if base.startswith("sub-") else "main"


def put(name: str, obj: Any) -> dict[str, Any]:
    """Publish one object on the family blackboard (D164) and return its sidecar.

    The object is dilled to ``<family>/<name>.dill`` with a ``<name>.json`` sidecar
    ``{name, owner, at, bytes, type, serializer}``; any member reads it back with
    ``get(name)`` and the host serves the sidecar at ``family://<name>``. A large
    result comes home this way, never through the transcript.
    """
    if not isinstance(name, str) or not FAMILY_NAME.match(name):
        raise ValueError("a blackboard name is a plain file-safe token, e.g. 'shard_auth'")
    directory = _family_dir()
    directory.mkdir(parents=True, exist_ok=True)
    serializer = _serializer()
    target = directory / f"{name}.dill"
    tmp = directory / f"{name}.dill.tmp-{os.getpid()}"
    with open(tmp, "wb") as handle:
        serializer.dump(obj, handle)
    os.replace(tmp, target)
    sidecar = {
        "name": name,
        "owner": _member_name(),
        "at": time.time(),
        "bytes": target.stat().st_size,
        "type": type(obj).__name__,
        "serializer": serializer.__name__,
    }
    (directory / f"{name}.json").write_text(json.dumps(sidecar))
    return sidecar


def get(name: str) -> Any:
    """Read one blackboard object back; ``KeyError`` names a missing entry."""
    if not isinstance(name, str) or not FAMILY_NAME.match(name):
        raise ValueError("a blackboard name is a plain file-safe token, e.g. 'shard_auth'")
    target = _family_dir() / f"{name}.dill"
    if not target.exists():
        raise KeyError(f"no blackboard entry {name!r}; rlm.ls() lists what members put")
    with open(target, "rb") as handle:
        return _serializer().load(handle)


def ls() -> list[dict[str, Any]]:
    """Every blackboard sidecar, oldest first."""
    directory = _family_dir()
    if not directory.is_dir():
        return []
    entries = []
    for sidecar in directory.glob("*.json"):
        try:
            entries.append(json.loads(sidecar.read_text()))
        except (OSError, ValueError):
            continue
    entries.sort(key=lambda entry: entry.get("at", 0))
    return entries


class BashHandle:
    """One backgrounded shell command; this handle owns the host-side job.

    ``await h`` waits for exit and returns the final report (``command``,
    ``output``, ``exit_code``, ``killed``); ``await h.kill()`` stops the job
    first and returns the same report. Both release the host-side job, so a
    handle needs no manual cleanup. ``await h`` has no deadline of its own —
    it polls in short host round trips, so wrap ``h.wait()`` in
    ``asyncio.wait_for`` (or ``asyncio.timeout``) when one is needed.
    """

    def __init__(self, command: str) -> None:
        if not isinstance(command, str) or not command.strip():
            raise ValueError("command must be a non-empty str")
        self._command = command
        self._cursor = 0
        self._report: dict[str, Any] | None = None
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            self._spawn: asyncio.Future[dict[str, Any]] | None = None
        else:
            self._spawn = loop.create_task(host_request("exec.spawn", {"command": command}))

    async def _id(self) -> int:
        if self._spawn is None:
            self._spawn = asyncio.get_running_loop().create_task(
                host_request("exec.spawn", {"command": self._command})
            )
        reply = await asyncio.shield(self._spawn)
        job = reply.get("job_id")
        if not isinstance(job, int):
            raise RuntimeError("exec.spawn returned an invalid job_id")
        return job

    async def tail(self) -> str:
        """New output since the last ``tail``; a released job's output lives on its report."""
        if self._report is not None:
            return ""
        chunk = await host_request("exec.tail", {"job_id": await self._id(), "cursor": self._cursor})
        next_cursor = chunk.get("next")
        if isinstance(next_cursor, int):
            self._cursor = next_cursor
        text = chunk.get("text")
        text = text if isinstance(text, str) else ""
        dropped = chunk.get("dropped")
        if isinstance(dropped, int) and dropped > 0:
            return f"[... {dropped} bytes trimmed from the front ...]\n{text}"
        return text

    async def poll(self) -> dict[str, Any]:
        """A snapshot — ``running``, ``exit_code``, ``killed`` — that never waits."""
        if self._report is not None:
            return {
                "running": False,
                "exit_code": self._report.get("exit_code"),
                "killed": self._report.get("killed", False),
            }
        return await host_request("exec.poll", {"job_id": await self._id()})

    async def kill(self) -> dict[str, Any]:
        """Stop the job if it still runs, release it, and return the final report."""
        if self._report is None:
            await host_request("exec.kill", {"job_id": await self._id()})
        return await self.wait(poll=0.05)

    async def wait(self, poll: float = 0.5) -> dict[str, Any]:
        """Wait for exit, release the host-side job, and return the final report."""
        while self._report is None:
            status = await self.poll()
            if self._report is not None:
                break
            if not status.get("running"):
                self._report = await host_request("exec.release", {"job_id": await self._id()})
                break
            await asyncio.sleep(poll)
        return self._report

    def __await__(self):
        return self.wait().__await__()

    def __repr__(self) -> str:
        if self._report is None:
            return f"BashHandle(command={self._command!r}, live)"
        return (
            f"BashHandle(command={self._command!r}, released, "
            f"exit_code={self._report.get('exit_code')!r}, killed={self._report.get('killed')!r})"
        )

    def __del__(self) -> None:
        spawn = getattr(self, "_spawn", None)
        if getattr(self, "_report", None) is not None or spawn is None:
            return

        async def _reap() -> None:
            try:
                job = (await asyncio.shield(spawn)).get("job_id")
                if isinstance(job, int):
                    await host_request("exec.kill", {"job_id": job})
                    await host_request("exec.release", {"job_id": job})
            except Exception:
                pass

        try:
            asyncio.get_running_loop().create_task(_reap())
        except RuntimeError:
            pass


def bash(command: str) -> BashHandle:
    """Start a shell command host-side and return its handle without waiting.

    The latency-hiding tool of first resort: ``h = bash("cargo build")``
    overlaps the build with everything else this cell does, then ``await
    h.tail()`` peeks, ``await h.poll()`` checks, ``await h.kill()`` stops,
    and ``await h`` collects. The job runs on the host under its broker —
    never as a kernel-side child process.
    """
    return BashHandle(command)


class _HarnessProxy:
    """Resolve the harness state against the current environment on every access.

    The kernel forkserver preimports rlm in a template process before per-session
    env vars exist; a state bound at import time would freeze that (env-less)
    resolution into every forked kernel. Resolving per access picks up the env
    applied after fork. Resolution must never raise (a failure inside the kernel
    namespace would take down the kernel). When the local store is genuinely
    unconfigured (no session env, e.g. --no-session) reads see an empty view but
    local writes raise instructively instead of vanishing on kernel exit; any
    other resolution failure degrades to a shared in-memory store until local
    resolution starts succeeding.
    """

    _fallback: HarnessState | None = None
    _unpersisted: HarnessState | None = None

    def _resolve(self) -> HarnessState:
        try:
            return get_harness_state()
        except RuntimeError as exc:
            if "Local harness state requires" in str(exc):
                if _HarnessProxy._unpersisted is None:
                    _HarnessProxy._unpersisted = HarnessState(
                        in_memory=True,
                        local_write_error=(
                            f"{exc} This session has no persistent local harness store; "
                            "pass global_=True to persist across sessions."
                        ),
                    )
                return _HarnessProxy._unpersisted
            return self._degraded()
        except Exception:  # pragma: no cover - harness access must never raise
            return self._degraded()

    @staticmethod
    def _degraded() -> HarnessState:
        if _HarnessProxy._fallback is None:
            _HarnessProxy._fallback = HarnessState(in_memory=True)
        return _HarnessProxy._fallback

    def __getattr__(self, name: str) -> Any:
        return getattr(self._resolve(), name)

    def __repr__(self) -> str:
        return repr(self._resolve())


_harness_state = _HarnessProxy()


class _RLMCallable:
    harness = _harness_state
    get_harness_state = staticmethod(get_harness_state)

    async def run(self, prompt: str, **kwargs: Any) -> RLMSpawnHandle:
        return await run(prompt, **kwargs)

    async def fetch(self, url: str, *, as_text: bool = False) -> Any:
        return await fetch(url, as_text=as_text)

    def put(self, name: str, obj: Any) -> dict[str, Any]:
        return put(name, obj)

    def get(self, name: str) -> Any:
        return get(name)

    def ls(self) -> list[dict[str, Any]]:
        return ls()

    def bash(self, command: str) -> BashHandle:
        return bash(command)

    async def find_models(self, query: str = "", limit: int = 8) -> list[RLMModel]:
        return await find_models(query, limit)

    async def list_subagents(self) -> list[RLMSubagent]:
        return await list_subagents()

    async def delete_subagent(self, target: str | RLMSubagent) -> RLMSubagent:
        return await delete_subagent(target)

    async def send(self, target: str | RLMSubagent, message: str, followup: bool = False) -> dict[str, Any]:
        return await send(target, message, followup)

    async def followup(self, target: str | RLMSubagent, message: str) -> dict[str, Any]:
        return await followup(target, message)

    async def list_agents(self) -> list[dict[str, Any]]:
        return await list_agents()

    async def status(self, name: str | None = None) -> list[dict[str, Any]]:
        return await status(name)

    async def wait(self, timeout: float = 300.0, cursor: int | None = None) -> dict[str, Any]:
        return await wait(timeout, cursor)

    async def plan_op(
        self,
        op: str,
        args: dict[str, Any] | None = None,
        *,
        plan: str | None = None,
        request_id: str | None = None,
        expected_revision: int | None = None,
    ) -> dict[str, Any]:
        return await plan_op(
            op, args, plan=plan, request_id=request_id, expected_revision=expected_revision
        )

    async def interrupt(self, target: str | RLMSubagent) -> dict[str, Any]:
        return await interrupt(target)

    async def result(
        self, target: str | RLMSubagent, *, schema: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        return await result(target, schema=schema)

    async def merge_worktree(self, target: str | RLMSubagent) -> dict[str, Any]:
        return await merge_worktree(target)

    async def discard_worktree(self, target: str | RLMSubagent) -> dict[str, Any]:
        return await discard_worktree(target)

    async def __call__(self, prompt: str, **kwargs: Any) -> RLMSpawnHandle:
        return await run(prompt, **kwargs)


rlm = _RLMCallable()
harness = _harness_state


class _CallableModule(types.ModuleType):
    async def __call__(self, prompt: str, **kwargs: Any) -> RLMSpawnHandle:
        return await run(prompt, **kwargs)


sys.modules[__name__].__class__ = _CallableModule

__all__ = [
    "BashHandle",
    "HarnessEntry",
    "HarnessScope",
    "HarnessState",
    "McpIntegration",
    "McpToolError",
    "NotEnabled",
    "RLMModel",
    "RLMSpawnHandle",
    "RLMSubagent",
    "RefinementEvent",
    "bash",
    "delete_subagent",
    "discard_worktree",
    "fetch",
    "find_models",
    "get_harness_state",
    "followup",
    "harness",
    "host_request",
    "interrupt",
    "list_agents",
    "list_subagents",
    "merge_worktree",
    "plan_op",
    "result",
    "rlm",
    "run",
    "send",
    "wait",
]

# Lazily re-export the MCP base class. Kept lazy so `import rlm` never requires
# the optional `mcp` SDK — only integration packages that subclass it do.
_LAZY_MCP = {"McpIntegration", "McpToolError", "NotEnabled"}


def __getattr__(name: str) -> Any:  # noqa: D401 - module-level lazy attr hook
    if name in _LAZY_MCP:
        from . import mcp_base

        return getattr(mcp_base, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
