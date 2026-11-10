---
decision: Skip source-only checks only in the derived public mirror | its transform removes mirror scanners, allowlists, and original history; source checkouts must fail when required inputs are missing | restore the omitted source inputs and run the full checks in the unfiltered checkout
---
Derived public mirrors now announce skips for checks that require omitted source history or scanner/mirror inputs. Full source checkouts still run those checks; absent inputs in a source checkout remain failures.
