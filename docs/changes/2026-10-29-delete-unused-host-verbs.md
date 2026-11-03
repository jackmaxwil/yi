---
issue: Closes #1530
---
The kernel host registry no longer registers `mcp.config`, `mcp.refresh` and `plan.get`. Python calls only `mcp.connect`; the three deleted verbs had no Python or Rust caller (the RPC `plan` route reads the plan service directly), so a cell that requests one still gets the "not available in this session" error it got from any unregistered verb. `HostRegistry::register_mcp_stubs` and `PlanService::register` are gone with them.
