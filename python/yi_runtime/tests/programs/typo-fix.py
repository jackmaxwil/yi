"""The typo-fix walkthrough as a program: one edit opens no plan, and nothing adopts one.

Its JSON twin is `crates/runtime/tests/fixtures/plans/typo-fix-never-opens-a-plan.json`.
"""
from yi import Plan, PlanError

try:
    await Plan.attach("fix-the-typo-in-readme-md")
except PlanError as refusal:
    print("no plan to join:", refusal.kind)
