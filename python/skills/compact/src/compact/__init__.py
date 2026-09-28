"""Yi compact skill: context compaction control from the kernel.

Compaction runs host-side (the same implementation as /compact); these
functions are thin typed wrappers over the generic host bridge
(`rlm.host_request`). They only work inside the Yi IPython kernel.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def status() -> dict[str, Any]:
    """Read current context usage.

    Returns a dict with `tokens`, `context_window`, `percent` (None right
    after a compaction until the next model response), and `scheduled`
    (whether a requested compaction is already pending for this turn).
    """
    return await host_request("compact.status")


async def run(instructions: str | None = None) -> dict[str, Any]:
    """Schedule context compaction.

    Compaction never runs mid-cell: it runs when the current turn ends and
    the harness resumes you automatically afterwards. Returns
    `{"scheduled": True}`, or `{"scheduled": False, "reason": ...}` when
    there is nothing to compact. Optional `instructions` focus the summary on
    what matters for the remaining work.
    """
    if instructions is not None and not isinstance(instructions, str):
        raise TypeError(f"instructions must be str or None, got {type(instructions).__name__}")
    payload: dict[str, Any] = {}
    if instructions is not None:
        payload["instructions"] = instructions
    return await host_request("compact.run", payload)


async def recall(pattern: str, limit: int | None = None, offset: int | None = None) -> dict[str, Any]:
    """Search the full session log for turns the summary cites as (#entryId).

    The compacted window holds a summary plus recent turns; the log keeps
    everything. `recall` greps that log (case-insensitive substring) and
    returns `{"hits": [{"entryId", "type", "snippet"}], "total": N}`, oldest
    first, 8 hits a page by default and 32 at most; a cut page carries a
    `notice` naming the call for the next page.
    Pull the full entry with `await rlm.fetch(f"history://<session-id>/{entryId}")`.
    """
    payload: dict[str, Any] = {"pattern": pattern}
    if limit is not None:
        payload["limit"] = limit
    if offset is not None:
        payload["offset"] = offset
    return await host_request("history.grep", payload)


async def search(query: str, limit: int | None = None, offset: int | None = None) -> dict[str, Any]:
    """Rank turns from every root session of this repository, lanes and worktrees included.

    BM25 over user messages, assistant text and compaction summaries; tool
    calls and their results are not indexed. Returns `{"hits": [{"session",
    "entryId", "type", "snippet", "url"}], "total": N}`, best first, 8 a page
    by default and 32 at most; a cut page carries a `notice` naming the call
    for the next one. Read a hit with `await rlm.fetch(hit["url"])`.
    """
    payload: dict[str, Any] = {"query": query}
    if limit is not None:
        payload["limit"] = limit
    if offset is not None:
        payload["offset"] = offset
    return await host_request("history.search", payload)
