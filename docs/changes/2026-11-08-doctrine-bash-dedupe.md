---
issue: Closes #2103
cut: crates/runtime/src/prompts/doctrine.md | Outside yolo mode every command runs contained where a sandbox exists. | the bash tool description already states every fact in it, and the system prompt paid for it twice on every request
---
The doctrine no longer restates the bash sandbox paragraph. Every fact it held (what leaves the sandbox, the "always" rule, the writable roots, the proxy, no unix socket) was already in the bash description. The two sentences with no twin moved there: a test that binds a unix socket fails inside the sandbox, and the repository's CI is where the answer lives. The system prompt is 544 bytes shorter and the bash description 96 bytes longer.
