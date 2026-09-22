"""The campaign walkthrough as a program: delegate, decompose, change course, supersede.

Its JSON twin is `crates/runtime/tests/fixtures/plans/campaign-decompose-and-supersede.json`;
a key here is that fixture's label in lower case. The fixture's refused steps are its own.
"""
from yi import Plan, Writer, cmd, contract

plan = await Plan.create("Ship logrotate-lite with a packaged tarball", request_id="campaign")
stub = await plan.todo(key="create-the-declared-tarball-path-with-a-stub")
freeze = await plan.todo(key="freeze-the-cli-argument-surface")
manpage = await plan.todo(
    key="write-the-manpage",
    delegate=Writer(
        accept=contract(cmd("man -l logrotate-lite.1", critical=True)),
        isolation=None,
        effort="low",
        context=["local://README.md"],
    ),
)
rotation = await plan.todo(key="implement-rotation-with-size-and-age-triggers", after=[freeze])
tests = await plan.todo(
    key="write-the-test-suite",
    after=[freeze],
    delegate=Writer(accept=contract(cmd("pytest -q tests/", critical=True)), effort="med"),
)
await plan.todo(key="repackage-the-tarball-from-the-finished-build", after=[stub, rotation, tests])

await manpage.start()
await stub.start()
await stub.done("local://dist/logrotate-lite.tar.gz")
await freeze.start()
await freeze.done("kernel://main/cli_surface")
await manpage.fail("the child documented the flags it invented rather than the frozen surface")
flag = await plan.todo(key="handle-the-compressed-rotation-flag", after=[rotation])
await tests.start()
await rotation.start()
sub = await rotation.decompose(
    [{"key": "rotate-on-a-size-threshold"}, {"key": "rotate-on-an-age-threshold", "after": ["rotate-on-a-size-threshold"]}]
)
size = sub["rotate-on-a-size-threshold"]
await size.start()
await size.fail("the size check raced the writer and truncated a partial line")
await size.retry(delegate=Writer(accept=contract(cmd("true", critical=True)), isolation=None, effort="high"))
await flag.block({"external": {"probe": "test -e /app/vendor/zstd"}}, "the image has no zstd; the vendor mount is still syncing")
await flag.unblock()
await flag.cancel()

await plan.supersede("the tarball must be produced by the project's own Makefile, not repackaged after the fact")
target = await plan.todo(key="write-the-makefile-dist-target")
dist = await plan.todo(key="drive-the-whole-build-through-make-dist", after=[target])
await target.start()
await target.done()
await dist.start()
await dist.done("local://dist/logrotate-lite.tar.gz")
