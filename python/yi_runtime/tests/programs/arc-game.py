"""The arc-game walkthrough as a program: one todo per level, a dead hypothesis, a rule only you have.

Its JSON twin is `crates/runtime/tests/fixtures/plans/arc-game-at-level-grain.json`; a key
here is that fixture's label in lower case.
"""
from yi import Plan

plan = await Plan.create("Reach the deepest level of ls20 that the rules allow", request_id="arc-game")
one = await plan.todo(key="reach-level-1")
two = await plan.todo(key="reach-level-2", after=[one])
three = await plan.todo(key="reach-level-3", after=[two])

await one.start()
await one.done()
await two.start()
await two.fail("the colour-swap hypothesis predicted the frame and the frame disagreed on every probe after the third")
await two.retry()
await two.start()
await two.done()
await three.start()
await three.block("user", "the level needs a rule the transcript never states and no probe can settle")
await three.unblock()
await three.start()
await three.done()
