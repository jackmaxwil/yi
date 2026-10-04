---
issue: Closes #939, closes #929
raise: tests +215, comments +13, crate cli +7, crate runtime +58, crate tools +74
---
A nested `sandbox-exec` that cannot apply its profile is no longer read as a refused write to `<cwd>/sandbox_apply` when the profile denies the cwd (Closes #939): it stays the pathless refusal a nested sandbox always was, and the bash result says what failed. Loopback-era network refusals now name the sandbox wherever the model meets them (Closes #929): a kernel cell, and a `bash()` job inside one, get a note that the kernel cannot leave the sandbox; `ping` gets a hint that names ICMP and exit 2 no longer hides it; a refusal behind an exit-0 pipe or `|| true` is found when a program that could use the network ran; and `yi doctor` lists the TCP listeners a contained process can reach.
