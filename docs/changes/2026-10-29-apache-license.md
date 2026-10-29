---
issue: Closes #1754
---
Yi is now licensed under the Apache License 2.0 instead of MIT. `LICENSE` holds the standard Apache text unmodified, and the new `NOTICE` carries the copyright notice for Jack Mazac and AiBridge LLC. The workspace manifest declares `Apache-2.0`, which every crate inherits, and the README and CONTRIBUTING say so. A release tarball now carries `LICENSE` and `NOTICE` beside the README, which it did not before. The files under `vendor/` and the code derived from ratatui keep their own upstream licenses, and the dependency allowlist in `deny.toml` is unchanged because dependencies are judged by their own licenses.
