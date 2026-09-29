"""Yi's kernel-side runtime shim (module name `rlm`).

A mail call (``send``, ``followup``, ``interrupt``, ``revoke``) returns a task already
running on the kernel loop, so an un-awaited ``rlm.send(...)`` goes out the moment the cell
next yields. Every other call returns a coroutine: ``await rlm.status()`` gets its value, and
one the cell never starts runs once after the cell, which prints its value with a note.
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import inspect
import json
import os
import sys
import pathlib
import threading
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

_PUBLIC: list[str] = []
_UNSETTLED: list[Any] = []
_CALL: list[type] = []
_MAIL: list[Any] = []
# Mail verbs whose effect is the point run at once; a spawn or a wait waits for its await.
_EAGER = frozenset({"send", "followup", "interrupt", "revoke"})
# The host's refusal of a wait repeated at an `asks` or `settled` state nothing has moved.
_REPEATED = ("is asking you", "stop waiting")
# A helper's own read: the host counts a question it quotes as shown only to the model.
_QUIET = contextvars.ContextVar("rlm_quiet", default=False)
# The host's refusal of a target that names no child, raised as NoSuchChild.
_NO_SUCH_CHILD = "No RLM child matches"


class NoSuchChild(RuntimeError, KeyError):
    """No child answers to that name; ``except KeyError`` and ``except RuntimeError`` both catch it."""

    __str__ = RuntimeError.__str__


def _ms(timeout: Any) -> int:
    """Seconds as the host's milliseconds; a negative or non-numeric timeout is refused, not replaced."""
    if not isinstance(timeout, (int, float)) or not 0 <= timeout < float("inf"):
        raise ValueError(f"timeout must be a number of seconds, 0 or more; got {timeout!r}")
    return int(timeout * 1000)


def _call_type() -> type:
    """The task class a mail call returns, built on whatever ``asyncio.Task`` is at first use
    (the kernel's ``nest_asyncio`` replaces it)."""
    if not _CALL:

        class RLMCall(asyncio.Task):  # type: ignore[misc, valid-type]
            """A mail call already running. Awaiting it, gathering or waiting on it, or reading
            its result takes it, and only a call nobody took is named after the cell."""

            taken = False

            def __await__(self):  # type: ignore[override]
                self.taken = True
                return super().__await__()

            __iter__ = __await__

            def add_done_callback(self, fn: Any, *, context: Any = None) -> None:
                self.taken = True
                super().add_done_callback(fn, context=context)

            def result(self) -> Any:
                self.taken = True
                return super().result()

        _CALL.append(RLMCall)
    return _CALL[0]


def _unsettled(call: Any, owned: set[Any]) -> bool:
    if isinstance(call, asyncio.Task):
        return not call.taken
    return inspect.getcoroutinestate(call) == inspect.CORO_CREATED and call not in owned


def _settle(*_: Any) -> None:
    """IPython's post_run_cell hook: finish each rlm call the cell never took and name it.

    A mail task still running is waited for; a coroutine never started runs once here.
    Invariant: an interrupt while one runs cancels it and every call after it, never leaving
    them to run on unreported.
    """
    pending, _UNSETTLED[:] = _UNSETTLED[:], []
    loop = asyncio.get_event_loop()
    owned = {task.get_coro() for task in asyncio.all_tasks(loop)}
    calls = [call for call in pending if _unsettled(call, owned)]
    for at, call in enumerate(calls):
        eager = isinstance(call, asyncio.Task)
        name = (call.get_coro() if eager else call).__qualname__
        label = f"{name}()" if "." in name else f"rlm.{name}()"
        calls[at] = task = asyncio.ensure_future(call, loop=loop)
        try:
            said = f"returned {loop.run_until_complete(task)!r}"
        except Exception as error:
            said = f"raised {type(error).__name__}: {error}"
        except BaseException:
            for rest in calls[at:]:
                if isinstance(rest, asyncio.Task):
                    rest.cancel()
                else:
                    rest.close()
            raise
        got, ran = ("a task", "as the cell went on") if eager else ("a coroutine object", "once after the cell")
        print(
            f"{label} was not awaited, so the cell got {got}, not its value; it ran {ran} and "
            f"{said}. Put `await` before the call to use it in the cell."
        )


def _handed_out(fn: Any) -> Any:
    """A mail verb's call is a task scheduled on the running loop; any other call is its
    coroutine. In a kernel either is recorded for ``_settle``."""

    @functools.wraps(fn)
    def call(*args: Any, **kwargs: Any) -> Any:
        made = fn(*args, **kwargs)
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = None
        if loop is not None and fn.__name__ in _EAGER:
            made = _call_type()(made, loop=loop)
            _MAIL[:] = [*(task for task in _MAIL if not task.done()), made]
        elif loop is not None:
            made = _after_mail(made)
        shell = get_ipython() if get_ipython is not None else None
        if shell is not None:
            if _settle not in shell.events.callbacks["post_run_cell"]:
                shell.events.register("post_run_cell", _settle)
            _UNSETTLED.append(made)
        return made

    return getattr(inspect, "markcoroutinefunction", lambda marked: marked)(call)


def _after_mail(coro: Any) -> Any:
    """``coro`` behind the mail calls made before it, which a first await would overtake."""

    async def behind() -> Any:
        if any(not task.done() for task in _MAIL):
            await asyncio.sleep(0)
        return await coro

    made = behind()
    made.__qualname__ = coro.__qualname__
    return made


async def _watch(timeout: float, cursor: int | None) -> "Reply | None":
    """``wait`` for a helper watching one child. The host refuses a wait repeated at an
    ``asks`` or ``settled`` state; the helper pauses on it and gets None, since it waits on
    its own child, not on that state."""
    # A budget's remainder is a clock difference that can land a hair under zero; wait refuses it.
    timeout = max(0.0, timeout)
    quiet = _QUIET.set(True)
    try:
        return await wait(timeout=timeout, cursor=cursor)
    except RuntimeError as error:
        if not any(mark in str(error) for mark in _REPEATED):
            raise
    finally:
        _QUIET.reset(quiet)
    await asyncio.sleep(max(0.0, min(0.5, timeout)))
    return None


class Reply(dict):
    """A reply that reads by key or by attribute: ``r["changed"]`` or ``r.changed``."""

    def __init__(self, data: dict[str, Any], call: str, example: str) -> None:
        super().__init__(data)
        self._call, self._example = call, example

    def _misuse(self, how: str) -> TypeError:
        keys = ", ".join(self.keys())
        return TypeError(f"{self._call} returns a dict ({keys}), which cannot be {how}; read it by key, e.g. {self._example}")

    def __getattr__(self, name: str) -> Any:
        if name.startswith("_"):
            raise AttributeError(name)
        try:
            return self[name]
        except KeyError:
            raise AttributeError(f"{self._call}'s reply has no {name!r}; its keys are {', '.join(self.keys())}") from None

    def __getitem__(self, key: Any) -> Any:
        if isinstance(key, int):
            raise self._misuse("indexed by position")
        return super().__getitem__(key)

    def __iter__(self) -> Any:
        raise self._misuse("unpacked or iterated (use .keys() or .items())")


def _public(fn: Any) -> Any:
    """One function of the ``rlm`` surface: listed in ``__all__``, its call recorded."""
    _PUBLIC.append(fn.__name__)
    return _handed_out(fn) if inspect.iscoroutinefunction(fn) else fn


def _quiet(payload: dict[str, Any]) -> dict[str, Any]:
    return {**payload, "quiet": True} if _QUIET.get() else payload


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

    @property
    def state(self) -> str:
        """This child's state now, asked of the host: ``(await rlm.status(name)).state``."""
        async def quietly() -> Reply:
            _QUIET.set(True)
            return await status.__wrapped__(self.name)

        read = quietly()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return asyncio.run(read).state
        if not hasattr(loop, "_nest_patched"):
            read.close()
            raise AttributeError(
                f"handle.state cannot block this loop; read (await rlm.status({self.name!r})).state"
            )
        return loop.run_until_complete(read).state

    def __repr__(self) -> str:
        head = (
            f"RLMSpawnHandle(name={self.name!r}, model={self.model!r}, "
            f"session_dir={str(self.session_dir)!r})"
        )
        return f"{head}\n{self.next}" if self.next else head

    @_handed_out
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
        reaps it) raises immediately instead of waiting to the deadline, and
        so does a child the host reports ``stuck``.
        """
        _check_schema(schema)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        # Invariant: a bare wait resumes from the model's last bare wait, which may already
        # have seen this child finish; cursor 0 reads the family as it stands now.
        cursor = 0
        while True:
            # Float rounding can put `(now + timeout) - now` an ulp past `timeout`.
            remaining = min(timeout, deadline - loop.time())
            if remaining <= 0:
                break
            reply = await _watch(remaining, cursor)
            if reply is None:
                continue
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
            if state == "stuck":
                notes = reply.get("notes")
                note = notes.get(self.name) if isinstance(notes, dict) else None
                raise RuntimeError(
                    f"child {self.name} is stuck ({note or 'no progress in its records'}); "
                    "rlm.send it a nudge, rlm.interrupt it, or call result() again to keep waiting"
                )
            if state in ("finished", "failed", "needs_you"):
                try:
                    return await result(self.rlm_child_id, schema=schema)
                except RuntimeError as error:
                    # Another run can start between the wait and the read; it is waited on.
                    if "still running" not in str(error):
                        raise
                    notes = reply.get("notes")
                    note = notes.get(self.name) if isinstance(notes, dict) else None
                    if isinstance(note, str) and note.startswith("asks "):
                        raise RuntimeError(
                            f"child {self.name} {note}; answer it with rlm.send({self.name!r}, "
                            "text, reply_to=<that id>), then call result() again"
                        ) from error
        raise TimeoutError(f"child {self.name} did not finish within {timeout}s")

    @_handed_out
    async def send(self, message: str, followup: bool = False) -> dict[str, Any]:
        return await send.__wrapped__(self.name, message, followup)


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
    if kernel is None:
        return
    comm_manager = getattr(kernel, "comm_manager", None)
    control_handlers = getattr(kernel, "control_handlers", None)
    if comm_manager is None or not isinstance(control_handlers, dict):
        # Without these every host reply is lost and the awaiting cell hangs; say so instead.
        raise RuntimeError(
            "this kernel's ipykernel has no kernel.comm_manager / kernel.control_handlers; "
            "Yi's host requests need ipykernel 7 (a YI_KERNEL_PYTHON interpreter must provide it)"
        )
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


@_public
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
                    refused = NoSuchChild if str(message).startswith(_NO_SUCH_CHILD) else RuntimeError
                    future.set_exception(refused(str(message)))
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


def _host_request_blocking(
    request_type: str, payload: dict[str, Any] | None = None, timeout: float = 600.0
) -> dict[str, Any]:
    """``host_request`` for synchronous callers; the reply lands on the control thread."""
    if Comm is None:
        raise RuntimeError("Jupyter comm support is unavailable in this kernel")
    _install_control_comm_handlers()
    done = threading.Event()
    reply: dict[str, Any] = {}

    def _on_msg(msg: dict[str, Any]) -> None:
        content = msg.get("content", {})
        data = content.get("data", {}) if isinstance(content, dict) else {}
        if isinstance(data, dict) and not done.is_set():
            reply.update(data)
            done.set()

    comm = Comm(target_name=HOST_COMM_TARGET, primary=False)
    comm.on_msg(_on_msg)
    comm.open(data={**(payload or {}), "type": request_type})
    try:
        if not done.wait(timeout):
            raise RuntimeError(f"host request {request_type} timed out after {timeout:.0f}s")
    finally:
        comm.close()
    if reply.get("status") == "ok":
        return {key: value for key, value in reply.items() if key != "status"}
    raise RuntimeError(str(reply.get("error") or f"host request {request_type} failed"))


# The host's per-value cap; the kernel sends one char past it so the host's clamp, the one
# place that writes the truncation marker, fires.
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
        context[key] = text[: CONTEXT_VALUE_CAP + 1]
    kwargs["context"] = context
    return kwargs


@_public
async def run(prompt: str, **kwargs: Any) -> RLMSpawnHandle:
    """Spawn a recursive Yi child and return once its task is admitted.

    ``model`` selects a child with an exact ``provider/model`` selector.
    ``thinking`` sets the child reasoning level (e.g. 'off', 'low', 'medium', 'high');
    defaults to the parent level; levels invalid for the resolved model fail the spawn.
    ``fork`` seeds the child with this session's history: 'none' (default), 'all'
    (inherits the parent model, so ``model``/``thinking`` are refused with it), or a
    positive turn count for the last N turns.
    ``isolation='worktree'`` gives the child its own checkout; hand it back with
    ``merge_worktree`` or ``discard_worktree``. ``isolation='container:<image>'`` is the
    same checkout and hand-back, with the child's bash run in a container of that image
    that mounts the checkout at the same path; its kernel stays on this machine.
    ``deny_write`` (and ``deny_read``) are lists of paths the child may not touch —
    the wall that keeps an implementer out of the standard it is measured against.
    ``deny_url`` is the same wall in URL space: a list of literal prefixes the
    child's ``fetch`` refuses, so ``["kernel://"]`` walls a whole scheme.
    ``context_keys`` is the child's whole view of this kernel: those variables are
    serialized into its brief and nothing else of this namespace reaches it.
    ``check`` makes it a protocol child — it owes a ``{"value": …, "discoveries":
    […]}`` answer, and ``result`` withholds that answer while the check is red.
    ``deadline_s`` and ``tokens`` are the child's lease, drawn from this session's own: an
    ask past what is left here is refused with both numbers, never clamped. ``parent_close``
    is ``"terminate"`` (default, 30 s grace) or ``"request_cancel"``; work is kept either way.
    A bare call makes a question-child (``role="reader"``, the default; ``role="root"`` is a
    full child with this session's prompt and tools): a short reader prompt instead of this session's,
    ``tools`` from ``["read", "grep"]`` (both by default, ``[]`` for one request), at most
    ``turns`` requests (3; the last is told to answer, and a tool call in it is refused), writes walled off, and no kernel. It stands
    outside the child cap; its finish reaches you like any child's unless ``result`` took it. ``partition`` is
    a list of URLs (``local://path#L1-40@TAG``, ``history://…``, ``plan://…``) resolved now and
    inlined into its brief as numbered, fenced lines, for any role; a kernel value rides
    ``context_keys``.
    """
    if not isinstance(prompt, str):
        raise TypeError(f"prompt must be str, got {type(prompt).__name__}")
    kwargs = _resolve_context(kwargs)
    payload = await host_request("rlm.run", {"prompt": prompt, "kwargs": kwargs})
    return _spawn_handle_from_payload(payload)


@_public
async def ask(
    question: str,
    partition: "list[str] | tuple[str, ...]" = (),
    *,
    schema: dict[str, Any] | None = None,
    timeout: float = 540.0,
    **kwargs: Any,
) -> dict[str, Any]:
    """Ask one question-child and return its answer; it is reaped either way.

        answers = await asyncio.gather(*(
            rlm.ask("Can this function panic? Quote the line.", [url], schema=PANIC) for url in urls))

    ``kwargs`` are ``run``'s (``tools``, ``turns``, ``model``, ``thinking``, ``context_keys``).
    """
    urls = [partition] if isinstance(partition, str) else list(partition)
    handle = await run(question, role="reader", partition=urls, **kwargs)
    try:
        return await handle.result(schema=schema, timeout=timeout)
    finally:
        await delete_subagent(handle)


@_public
async def service(name: str, brief: str, restart: int = 3, **kwargs: Any) -> RLMSpawnHandle:
    """Start a service: a child whose ``name`` is its address for as long as this session lives.

    It idles between turns and ``send`` or ``request`` wakes it. A run that crashes (a provider
    error, a dead kernel) is respawned under the same name with its transcript and inbox kept
    and a fresh lease, at most ``restart`` times in ten minutes (the host refuses more than ten
    and counts them in memory, so a restarted host starts the count over); past that, or when
    the parent has no lease left to draw, it reads ``failed`` and the notice says why. Each
    incarnation is billed for its own turns only. The same ``name``
    and ``brief`` again attach to the running service; ``delete_subagent``, ``revoke`` and the
    parent's close end it for good. ``status()`` shows ``service`` and ``incarnation``; a
    message carries the incarnation it was addressed to. ``kwargs`` are ``run``'s, less
    ``name`` and ``isolation``.
    """
    if not isinstance(restart, int) or restart < 0:
        raise ValueError("restart is how many respawns are allowed in ten minutes: 0 or more")
    kwargs = _resolve_context(kwargs)
    payload = {"name": name, "prompt": brief, "restart": restart, "kwargs": kwargs}
    return _spawn_handle_from_payload(await host_request("rlm.service", payload))


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


@_public
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


@_public
async def list_subagents() -> list[RLMSubagent]:
    """List direct RLM children retained by the current parent session."""
    payload = await host_request("rlm.list_subagents")
    entries = payload.get("subagents")
    if not isinstance(entries, list):
        raise RuntimeError("rlm.list_subagents returned an invalid subagents registry")
    return [_subagent_from_payload(entry) for entry in entries]


@_public
async def delete_subagent(target: str | RLMSubagent | RLMSpawnHandle) -> RLMSubagent:
    """Delete one running or retained direct child from the current parent session."""
    if isinstance(target, (RLMSubagent, RLMSpawnHandle)):
        selector = target.rlm_child_id
    elif isinstance(target, str):
        selector = target.strip()
        if not selector:
            raise ValueError("target must not be empty")
    else:
        raise TypeError(
            f"target must be str, RLMSubagent or RLMSpawnHandle, got {type(target).__name__}"
        )
    payload = await host_request("rlm.delete_subagent", {"target": selector})
    return _subagent_from_payload(payload.get("subagent"), "rlm.delete_subagent")


def _mail(target: "str | RLMSubagent", message: str, **options: Any) -> dict[str, Any]:
    if not isinstance(message, str) or not message:
        raise ValueError("message must be a non-empty str")
    selector = target if isinstance(target, str) else target.session_name
    sent = {key: value for key, value in options.items() if value is not None}
    return {"target": selector, "message": message, **sent}


@_public
async def send(
    target: "str | RLMSubagent",
    message: str,
    followup: bool = False,
    *,
    reply_to: str | None = None,
    kind: str | None = None,
    ref: str | None = None,
    conversation: str | None = None,
    deadline_ms: int | None = None,
) -> dict[str, Any]:
    """Send one agent message; ``followup=True`` also starts the target's turn.

    ``target`` is an agent name, ``"parent"``, or ``"all"``. A broadcast returns
    one receipt per target rather than failing whole on the first bad one. Each
    receipt is ``{target, id, state, presented}``: ``state`` says what the host did
    once the message was in the target's inbox, ``queued`` (a running turn will take
    it), ``woken`` (a turn was started on it) or ``inboxed`` (it waits in the
    store; nothing is running to read it), and ``presented`` says when the target's
    model reads it. ``reply_to=<id>`` answers a request: the waiting call returns it
    (``answered``), and a request already answered refuses it. A plain send to a
    child with exactly one question open to you, once you were shown it, is that
    question's answer; before that it stays mail and its receipt carries a ``hint``.
    ``kind`` is ``inform`` (the default), ``progress``, ``failure`` or ``cancel``.
    A body over 16 KiB is refused, never trimmed: ``put`` it and pass
    ``ref="family://<name>"``.
    """
    options = {"reply_to": reply_to, "kind": kind, "ref": ref}
    options.update(conversation=conversation, deadline_ms=deadline_ms)
    return await host_request("agent_message.send", _mail(target, message, followup=bool(followup), **options))


@_public
async def request(target: "str | RLMSubagent", message: str, timeout: float = 300.0) -> dict[str, Any]:
    """Send a request and wait for its reply; an idle target is started on it.

    Returns ``{reply, envelope, receipts}``: the reply's text, its whole envelope
    and the request's receipt. Only the target's own
    ``send(sender, text, reply_to=<id>)`` resolves it. RuntimeError when no reply
    came within ``timeout`` seconds; a late reply still lands in your history.
    """
    return await host_request("agent_message.request", _mail(target, message, timeout_ms=_ms(timeout)))


@_public
async def subscribe(
    address: str,
    create: dict[str, Any],
    *,
    filter: str | None = None,
    batch: int | None = None,
    window: float | None = None,
    min_interval: float | None = None,
    retention: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Make a todo from what arrives at ``address``; returns ``{job}``.

    ``address`` is ``clock://<schedule>``, ``channel://<name>`` or a source URI:
    ``exec://<command>?every=30s`` (its exit, ``{ok, exit, output}``, when it changes),
    ``file://<path>`` (its ``{exists, size, mtimeMs}``), or ``<scheme>://…`` for an
    installed ``yi-adapter-<scheme>``. ``create`` is ``{label, note, intent?}``: a match
    appends one todo and wakes you with the messages as data, never as a prompt.
    ``filter`` is ``key=value`` terms joined by ``&``, or a substring. At most ``batch``
    messages (20) ride one todo, at most one a ``window``/``min_interval`` (60 s).
    ``retention`` (``{count}`` or ``{age}`` seconds; 256 messages) is set by the first
    subscription to a source. To wait instead, block a todo ``on`` the address.
    """
    if not isinstance(create, dict) or not isinstance(create.get("label"), str):
        raise TypeError("create must be a dict with a label")
    payload: dict[str, Any] = {
        "address": address,
        "label": create["label"],
        "prompt": create.get("note") or create["label"],
    }
    if "intent" in create:
        payload["intent"] = create["intent"]
    for key, value in (("filter", filter), ("batch", batch)):
        if value is not None:
            payload[key] = value
    for key, seconds in (("windowMs", window), ("minIntervalMs", min_interval)):
        if seconds is not None:
            payload[key] = _ms(seconds)
    if retention is not None:
        kept = {"count": retention.get("count")}
        if retention.get("age") is not None:
            kept["ageMs"] = _ms(retention["age"])
        payload["retention"] = {key: value for key, value in kept.items() if value is not None}
    return await host_request("rlm_heartbeat.create", payload)


@_public
async def receive(timeout: float = 300.0) -> list[dict[str, Any]]:
    """Block until mail for you arrives; returns every envelope not yet shown to you.

    Envelopes come in send order, each ``{id, from, to, kind, conversation, inReplyTo,
    seq, sentAt, body, ref}``; answer a ``request`` with ``send(env["from"], text,
    reply_to=env["id"])``. What this returns is never presented to you again. An empty
    list means ``timeout`` seconds passed; the host clamps it to 1 to 300 seconds. An
    envelope reads by key or attribute::

        for env in await rlm.receive(60):
            print(env["from"], env.body)
    """
    payload = await host_request("rlm.receive", {"timeout_ms": _ms(timeout)})
    envelopes = payload.get("envelopes")
    if not isinstance(envelopes, list):
        raise RuntimeError("rlm.receive returned an invalid envelope list")
    example = 'for env in await rlm.receive(60): print(env["from"], env.body)'
    return [Reply(env, "rlm.receive", example) if isinstance(env, dict) else env for env in envelopes]


@_public
async def followup(target: "str | RLMSubagent", message: str) -> dict[str, Any]:
    """Send and start the target's turn if it is idle (delivered at a boundary if not)."""
    return await send.__wrapped__(target, message, followup=True)


@_public
async def status(name: str | None = None) -> "list[Reply] | Reply":
    """Every child's state as its own records show it (D165).

    Each entry is ``{name, state, note, tools, tokens, idle_s, worktree}`` with ``state``
    one of ``queued`` (admitted, not yet started), ``running``, ``finished``, ``failed``
    (its run ended badly, or it sent you a ``failure`` of its own while still running),
    ``needs_you`` (it waits on your answer, ``note`` reading ``asks <id>: <question>``:
    reply with ``send(name, text, reply_to=id)``; or it blocked a todo on you),
    ``stuck`` (a repeat break, a length re-drive at rung two or more, a let-go
    intercept, or five idle minutes; ``note`` names which) and ``repossession_pending``
    (``revoke`` took its lease back but the stop, settle or record failed; everything it
    held is kept and the host retries). An entry reads by key or attribute. With
    ``name`` (a plan child's todo label names it too) it returns that one child's entry,
    and an unknown name raises NoSuchChild (a KeyError) naming the children there are::

        for m in await rlm.status():
            print(m["name"], m.state)
        if (await rlm.status("counter")).tools >= 1: ...
    """
    payload = await host_request("rlm.status", _quiet({} if name is None else {"name": name}))
    members = payload.get("members")
    if not isinstance(members, list):
        raise RuntimeError("rlm.status returned an invalid member list")
    example = 'for m in await rlm.status(): print(m["name"], m.state)'
    entries = [Reply(member, "rlm.status", example) for member in members if isinstance(member, dict)]
    if name is None:
        return entries
    named = [entry for entry in entries if entry.get("name") == name]
    named = named or [entry for entry in entries if str(entry.get("name")).endswith(f"/{name}")]
    if len(named) == 1:
        return named[0]
    roster = (await host_request("rlm.status", {"quiet": True})).get("members") or []
    known = ", ".join(repr(member.get("name")) for member in roster if isinstance(member, dict))
    raise NoSuchChild(f"no one child named {name!r}; the children are: {known or 'none'}")


@_public
async def list_agents() -> list[dict[str, Any]]:
    """The family this agent can address by name."""
    payload = await host_request("agent_message.list_agents", {})
    agents = payload.get("agents")
    if not isinstance(agents, list):
        raise RuntimeError("agent_message.list_agents returned an invalid roster")
    return agents


@_public
async def wait(timeout: float = 300.0, cursor: int | None = None) -> dict[str, Any]:
    """Block until a child reports or finishes; returns what moved since ``cursor``.

    The reply carries ``cursor`` (pass it back on the next call and no other
    waiter can steal your updates), ``changed`` and its one-release alias
    ``updated`` (the names that moved), ``causes`` (why each moved, by name:
    ``mail``, ``finished``, ``asked``, ``reaped`` and the like), ``states`` (every
    registered child by name: ``running``, ``finished``, ``failed``, ``needs_you``
    or ``stuck``) and ``notes``. With no cursor the host keeps your last one: the first call
    answers at once with the family as it stands, each later one blocks until a child moves.
    ``state`` says why it returned: ``moved``, ``timeout``, ``asks`` (a child waits on
    your answer, cause ``asks``) or ``settled`` (nothing in the family is live, with
    ``finished``). ``asks`` and ``settled`` return at once, once: a wait repeated at the
    same state with nothing moved raises RuntimeError naming it, so answer the question, or
    stop waiting on a family with nothing running. The host clamps the timeout and says so
    in the reply (``clamped``). The reply reads by key or attribute::

        r = await rlm.wait(300)
        print(r["state"], r.changed, r.states)
    """
    payload: dict[str, Any] = {"timeout_ms": _ms(timeout)}
    if cursor is not None:
        payload["cursor"] = cursor
    reply = await host_request("rlm.wait", _quiet(payload))
    return Reply(reply, "rlm.wait", 'r = await rlm.wait(300); print(r["state"], r.changed)')


@_public
async def plan_op(
    op: str,
    args: dict[str, Any] | None = None,
    *,
    plan: str | None = None,
    request_id: str | None = None,
    expected_revision: int | None = None,
    artifacts: list[dict[str, str]] | None = None,
) -> dict[str, Any]:
    """Run one op against the host's plan engine and return its reply.

    The same parser and the same engine the JSON plan tool uses; the host
    fixes the actor from the registry this kernel is wired to, so a child
    kernel gets its parent's plan read-only whatever it asks for. ``plan``
    names a plan other than the session's and ``expected_revision`` refuses
    the op if the plan moved under you. Pass your own ``request_id`` to retry
    as the same request; without one each call is minted a new id and is a
    new request. Read ``ok`` in the reply: a refusal is data, not an
    exception; ``plan`` is the plan as it stands. ``artifacts`` are
    ``{media_type, text}`` blobs stored in the plan before the op applies,
    which the op's args then name by sha256 (``help(yi)`` builds them).

        reply = await plan_op("view", {"full": True})
    """
    payload: dict[str, Any] = {
        "request_id": request_id or str(uuid.uuid4()),
        "plan": plan,
        "expected_revision": expected_revision,
        "op": op,
        "args": args,
        "artifacts": artifacts or None,
    }
    return await host_request("plan.op", {k: v for k, v in payload.items() if v is not None})


@_public
async def interrupt(target: "str | RLMSubagent") -> dict[str, Any]:
    """End a child's run and keep its record (``delete_subagent`` reaps instead)."""
    return await host_request("rlm.interrupt", {"target": _worktree_target(target)})


@_public
async def revoke(
    target: "str | RLMSubagent", *, grace_s: float = 30, reason: str = ""
) -> dict[str, Any]:
    """Take a running child's lease back: it is sent a ``cancel`` and has ``grace_s`` to stop.

    A child still running when the grace ends is repossessed: its run is stopped, its
    worktree's work is kept on its branch, the record lands in this session's history and
    ``wait`` stops listing it. One that stops in time keeps its record for ``result``.
    """
    payload = {"target": _worktree_target(target), "grace_ms": int(grace_s * 1000), "reason": reason}
    return await host_request("rlm.revoke", payload)


@_public
async def result(
    target: "str | RLMSubagent",
    *,
    schema: dict[str, Any] | None = None,
    timeout: float | None = None,
) -> dict[str, Any]:
    """A finished child's answer as data, checked against ``schema`` host-side.

    ``timeout`` waits that many seconds for a child that is still running, as
    ``RLMSpawnHandle.result`` does; omitted, a running child raises at once.
    """
    _check_schema(schema)
    payload: dict[str, Any] = {"target": _worktree_target(target)}
    if schema is not None:
        payload["schema"] = schema
    if timeout is None:
        return await host_request("rlm.result", payload)
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while True:
        try:
            return await host_request("rlm.result", payload)
        except RuntimeError as error:
            remaining = min(timeout, deadline - loop.time())
            if remaining <= 0 or "still running" not in str(error):
                raise
        await _watch(remaining, None)


def _worktree_target(target: "str | RLMSubagent") -> str:
    if isinstance(target, RLMSubagent):
        return target.rlm_child_id
    if isinstance(target, str):
        selector = target.strip()
        if not selector:
            raise ValueError("target must not be empty")
        return selector
    raise TypeError(f"target must be str or RLMSubagent, got {type(target).__name__}")


@_public
async def merge_worktree(target: "str | RLMSubagent") -> dict[str, Any]:
    """Commit an isolated child's work on its branch and merge it into this checkout.

    The child must have finished. The worktree is removed once merged, which is
    also what releases the child for ``delete_subagent``.
    """
    return await host_request("rlm.merge_worktree", {"target": _worktree_target(target)})


@_public
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


class Page(str):
    """One page of a paged ``fetch``: the text, and where the next page starts (None at the end)."""

    next_offset: int | None = None


@_public
async def fetch(
    url: str, *, as_text: bool = False, offset: int | None = None, limit: int | None = None
) -> Any:
    """Read one addressable URL; a URL names a noun, so a fetch never writes.

    ``kernel://<var>`` and ``kernel://main/<var>`` in the plan owner's kernel
    read this namespace directly and return the live object, with no host
    round trip. Every other URL — ``local://``, ``plan://``, ``history://``,
    ``checkpoint://``, ``mcp://``, another agent's ``kernel://`` — is resolved
    by the host and returns text. Batch reads with ``asyncio.gather``.

    ``offset`` and ``limit`` page a read: bytes of ``local://``, entries of
    ``history://``, chars of another agent's ``kernel://`` repr. A paged read
    returns a ``Page``, a str whose ``next_offset`` continues it until None.
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
    paged = {key: value for key, value in (("offset", offset), ("limit", limit)) if value is not None}
    want_object = url.startswith("kernel://") and not as_text and not paged
    reply = await host_request("fetch", {"url": url, "object": want_object, **paged})
    path = reply.get("path")
    if want_object and isinstance(path, str):
        with open(path, "rb") as handle:
            return _serializer().load(handle)
    text = reply.get("text")
    if not isinstance(text, str):
        raise RuntimeError(f"fetch of {url} returned no text")
    if paged:
        text = Page(text)
        text.next_offset = reply.get("next_offset")
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


@_public
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
    # Invariant: both files are staged before either is renamed, so a put that fails while
    # writing leaves `get`, `ls` and family:// all on the previous pair.
    staged: list[tuple[Path, Path]] = []
    try:
        staged.append((_stage(target, lambda handle: serializer.dump(obj, handle)), target))
        sidecar = {
            "name": name,
            "owner": _member_name(),
            "at": time.time(),
            "bytes": staged[0][0].stat().st_size,
            "type": type(obj).__name__,
            "serializer": serializer.__name__,
        }
        encoded = json.dumps(sidecar).encode()
        sidecar_path = directory / f"{name}.json"
        staged.append((_stage(sidecar_path, lambda handle: handle.write(encoded)), sidecar_path))
        for tmp, path in staged:
            os.replace(tmp, path)
    except BaseException:
        for tmp, _ in staged:
            tmp.unlink(missing_ok=True)
        raise
    return sidecar


def _stage(path: Path, write: Any) -> Path:
    """Write a per-process tmp beside ``path`` for the caller to rename over it."""
    tmp = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    # Incident: a family member could plant a symlink at this predictable name, and open(..., "wb")
    # wrote through it to a file outside the board; O_EXCL|O_NOFOLLOW refuses any planted entry.
    tmp.unlink(missing_ok=True)
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "wb") as handle:
            write(handle)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    return tmp


@_public
def get(name: str) -> Any:
    """Read one blackboard object back; ``KeyError`` names a missing entry."""
    if not isinstance(name, str) or not FAMILY_NAME.match(name):
        raise ValueError("a blackboard name is a plain file-safe token, e.g. 'shard_auth'")
    target = _family_dir() / f"{name}.dill"
    if not target.exists():
        raise KeyError(f"no blackboard entry {name!r}; rlm.ls() lists what members put")
    with open(target, "rb") as handle:
        return _serializer().load(handle)


@_public
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
            self._spawn = asyncio.ensure_future(host_request("exec.spawn", {"command": command}), loop=loop)

    async def _id(self) -> int:
        if self._spawn is None:
            self._spawn = asyncio.ensure_future(host_request("exec.spawn", {"command": self._command}))
        reply = await asyncio.shield(self._spawn)
        job = reply.get("job_id")
        if not isinstance(job, int):
            raise RuntimeError("exec.spawn returned an invalid job_id")
        return job

    @_handed_out
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

    @_handed_out
    async def poll(self) -> dict[str, Any]:
        """A snapshot — ``running``, ``exit_code``, ``killed`` — that never waits."""
        if self._report is not None:
            return {
                "running": False,
                "exit_code": self._report.get("exit_code"),
                "killed": self._report.get("killed", False),
            }
        return await host_request("exec.poll", {"job_id": await self._id()})

    @_handed_out
    async def kill(self) -> dict[str, Any]:
        """Stop the job if it still runs, release it, and return the final report."""
        if self._report is None:
            await host_request("exec.kill", {"job_id": await self._id()})
        return await self.wait(poll=0.05)

    @_handed_out
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


@_public
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
harness = _harness_state


class _CallableModule(types.ModuleType):
    def __call__(self, prompt: str, **kwargs: Any) -> Any:
        return run(prompt, **kwargs)


sys.modules[__name__].__class__ = _CallableModule

__all__ = [
    *_PUBLIC,
    "BashHandle",
    "HarnessEntry",
    "HarnessScope",
    "HarnessState",
    "McpIntegration",
    "McpToolError",
    "NoSuchChild",
    "NotEnabled",
    "RLMModel",
    "RLMSpawnHandle",
    "RLMSubagent",
    "RefinementEvent",
    "get_harness_state",
    "harness",
]

# Lazily re-export the MCP base class. Kept lazy so `import rlm` never requires
# the optional `mcp` SDK — only integration packages that subclass it do.
_LAZY_MCP = {"McpIntegration", "McpToolError", "NotEnabled"}


def __getattr__(name: str) -> Any:  # noqa: D401 - module-level lazy attr hook
    if name in _LAZY_MCP:
        from . import mcp_base

        return getattr(mcp_base, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
