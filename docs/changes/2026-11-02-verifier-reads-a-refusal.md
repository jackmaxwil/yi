---
issue: Closes #1931
raise: crate runtime +14, comments +3, tests +189
---
A plan verifier's case runner now reads a sandbox refusal before the cap that cuts its output, so a refused runner abstains instead of reading as the product's `output_overflow`. A refusal that ends a run no longer unsays a case the product had already failed. A child's contained check is now tested by driving it to its result, not by its spawn returning.
