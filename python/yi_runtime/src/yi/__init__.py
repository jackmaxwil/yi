"""yi: plans as programs. ``help(yi.Plan)`` is the place to start.

    Plan      open, attach to or resume a plan; declare todos; run a scheduler
    Todo      one todo's ops: start, submit, done, fail, retry, cancel, result
    Run       a scheduler's lease: outcome, status, stop; launch and settle for shapes
    contract  group cmd, schema and example items into what ``done`` verifies
    Writer    a child that changes files behind a wall; Reader, one that only answers
    in_order  the default shape

The host decides everything: admission, step legality, verification. A refusal
raises ``PlanError`` (``Refused`` and ``Stale`` from ``done``, ``SpecDrift`` from
a changed redeclaration, ``RunActive`` from a second scheduler). Each cell's
source is recorded in the plan before its first effect and is never run again.

    from yi import Plan, contract, cmd
    plan = await Plan.create("ship logrotate-lite", request_id="create-01")
    await plan.todo(key="freeze", accept=contract(cmd("make -s check", critical=True)))
    run = await plan.run(budget="2h")
"""
from .contract import cmd, contract, example, schema
from .plan import Plan, PlanError, Refused, Run, RunActive, SpecDrift, Stale, Todo, in_order
from .roles import Reader, Writer

__all__ = [
    "Plan",
    "Todo",
    "Run",
    "in_order",
    "contract",
    "cmd",
    "schema",
    "example",
    "Writer",
    "Reader",
    "PlanError",
    "Refused",
    "Stale",
    "SpecDrift",
    "RunActive",
]
