---
issue: Closes #2107
cut: none Cargo.lock
raise: crate tools +53, tests +34
---
Every command starts about a third faster. `yi --version` retires 25.4M instructions instead of 38.3M and `yi ask` 100.9M instead of 154.8M (minimum of 30 interleaved dist runs each). Three cuts: ureq's `native-certs` feature is gone, so every HTTPS call except the cursor handshake and `yi update`'s keychain fallback (providers, OAuth logins, MCP HTTP servers, the classifier call, the uv download) trusts the bundled web roots only and CoreFoundation no longer loads at startup (an MCP server behind a private CA now fails with `UnknownIssuer`, and `SSL_CERT_FILE` is no longer read); `Spill::new` compiles its nine redact regexes only when a spill directory exists, which no captured git call has; and `host_program` stats a PATH entry before canonicalizing it. The dist binary grows 31,984 bytes because the web roots no longer dead-strip. The 6 MiB cap is gone (D553), so nothing gates that.
