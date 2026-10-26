---
issue: Refs #1484
---
Contain every kill runner yolo ask's environment. Only HOME, PATH, locale, timezone, proxy and CA settings cross from the invoking shell, so unrelated exported credentials cannot leak into a model run. The kill runner's README section states this contract beside its fresh-HOME isolation.
