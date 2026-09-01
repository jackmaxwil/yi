"""Yi's kernel-side runtime shim (module name `rlm`)."""

from __future__ import annotations

import asyncio
import json
import sys
import types
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
        schema: dict[str, Any] | None = None,
        timeout: float = 900.0,
        poll: float = 0.5,
    ) -> dict[str, Any]:
        """Wait for this child to finish and return its answer as data.

        The reply carries ``text`` and, when the child answered with JSON,
        ``json``. A ``schema`` is checked host-side and a mismatch raises,
        so a malformed result never reaches the parent's transcript as if
        it had passed.
        """
        deadline = 0.0
        while deadline < timeout:
            for entry in await list_subagents():
                if entry.rlm_child_id == self.rlm_child_id and entry.status != "running":
                    return await result(self.rlm_child_id, schema)
            await asyncio.sleep(poll)
            deadline += poll
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


async def list_agents() -> list[dict[str, Any]]:
    """The family this agent can address by name."""
    payload = await host_request("agent_message.list_agents", {})
    agents = payload.get("agents")
    if not isinstance(agents, list):
        raise RuntimeError("agent_message.list_agents returned an invalid roster")
    return agents


async def wait(timeout: float = 300.0) -> dict[str, Any]:
    """Block until a child reports or finishes; returns the names that moved.

    The host clamps the timeout and says so in the reply (``clamped``), so a
    caller is never silently given a different one.
    """
    return await host_request("rlm.wait", {"timeout_ms": int(timeout * 1000)})


async def interrupt(target: "str | RLMSubagent") -> dict[str, Any]:
    """End a child's run and keep its record (``delete_subagent`` reaps instead)."""
    return await host_request("rlm.interrupt", {"target": _worktree_target(target)})


async def result(target: "str | RLMSubagent", schema: dict[str, Any] | None = None) -> dict[str, Any]:
    """A finished child's answer as data, checked against ``schema`` host-side."""
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

    async def wait(self, timeout: float = 300.0) -> dict[str, Any]:
        return await wait(timeout)

    async def interrupt(self, target: str | RLMSubagent) -> dict[str, Any]:
        return await interrupt(target)

    async def result(
        self, target: str | RLMSubagent, schema: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        return await result(target, schema)

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
    "delete_subagent",
    "discard_worktree",
    "find_models",
    "get_harness_state",
    "followup",
    "harness",
    "host_request",
    "interrupt",
    "list_agents",
    "list_subagents",
    "merge_worktree",
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
