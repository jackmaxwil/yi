---
version: 0.940.0
issue: Closes #2033
---
The autofixer runs again. Its worker builds yi into its own `CARGO_TARGET_DIR`, while the review helper only looked for `target/debug/yi` under the checkout, so every fix died with "No such file or directory" and was deferred. The helper now looks for and returns yi where cargo builds it.
