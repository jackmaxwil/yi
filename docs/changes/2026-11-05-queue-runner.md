---
issue: Closes #2032
---
The merge queue's candidate push now runs its gate on the `queue` runner lane, a the runner VM runner that takes no other job, so a batch no longer waits behind pull request runs for a free `ubuntu-latest` slot. The candidate also saves its Rust build cache, since no main run reaches that runner's cache. Pull request runs keep `ubuntu-latest`, and an `external` PR still waits for `run-ci` on the `external` runner. The workflow lane check now reads a `runs-on` that picks its lane by branch, so the new lane is compared against review.yml's like the others.
