---
issue: Closes #TBD
raise: crate tools +47, tests +28
---
Every command starts about a third faster. `yi --version` retires 25.4M instructions instead of 38.3M and `yi ask` 100.9M instead of 154.8M (minimum of 30 interleaved dist runs each). Three cuts: ureq's `native-certs` feature is gone, so provider calls trust the bundled web roots only and CoreFoundation no longer loads at startup (a corporate proxy's keychain CA is no longer trusted for provider calls; the cursor handshake and `yi update` still read the system store); `Spill::new` compiles its nine redact regexes only when a spill directory exists, which no captured git call has; and `host_program` stats a PATH entry before canonicalizing it. The dist binary grows 31,984 bytes because the web roots no longer dead-strip. The 6 MiB cap is gone (D553), so nothing gates that.
