---
issue: Closes #601
decision: contained traffic leaves through a per-spawn CONNECT proxy, and named package caches are writable with their poisoning accepted | installs failed silently, and the owner left containment over it | remove the Spawn seam, the proxy and with_caches; amends D87, D205 and D331
growth: +1039 is the proxy (egress.rs: the head parser, the holder table, caps with rows, the relay), the broker's decision table, the Spawn seam through jobs, the kernel and checks, and the cache roots. Weighed for deletion: tokio for the proxy, refused since front ends run current_thread and attach_runtime is at its line cap; a dialer through an upstream proxy, cut until someone sits behind one; derivation of the allow-list from a lockfile, cut because a contained process writes the lockfile.
raise: tests +999, comments +78, crate cli +5, crate permission +9, crate runtime +968, crate tools +117, crate types +85
---
A contained command now has the network, through a loopback proxy that decides by host name. Registries and GitHub are seeded, `sandbox.egress` in the home config adds to them, "always" keeps a grant, and any other host asks a person once per spawn; headless, it is refused. The proxy reads no traffic: it tunnels, so TLS stays end to end. A raw connect is still refused by the sandbox, so a client that ignores the proxy variables fails rather than leaving unseen.

What the proxy refused reaches the model as a row beside the output: `[egress: refused example.com: not seeded or in sandbox.egress; asked you, retry after your answer]`. Bash, background jobs, the kernel's `bash()`, a kernel cell and a contained check all carry it.

A contained install writes `~/.cargo/{registry,git}`, `~/.cache/uv`, `~/.npm`, pnpm, pip and bun's stores, wherever `CARGO_HOME` and `UV_CACHE_DIR` put them, but never `.cargo/bin` or a config. This opens one thing on purpose: a contained process can edit a `build.rs` in the registry that your next terminal build runs. It is the price of installs that work, and `docs/ARCHITECTURE.md` carries it with the other residuals.

`sanitize` also strips bidi format controls, so a hostile name in a row or an ask reads in the order its bytes run.
