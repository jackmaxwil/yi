---
id: bloat
question: is it the smallest change that does the job
decider: reader
claim: Files edited
severity: {"high": "a parallel mechanism beside an existing one, or new public surface (a dependency, command, flag, config key, env var, schema field, file format) the why does not need", "medium": "complexity the job does not need: an abstraction with one user, a layer or option for a value that never varies, a special case where one guard in the shared function would do, dead code, or one concern spread across files that one place could hold"}
refute: {"high": 3, "medium": 1}
---
Judge the footprint against the why. Every new file, module, type, option, layer and indirection
must earn its place: ask what breaks if it is deleted or inlined, and report it when nothing
does. Look for sprawl: one concern spread over several files, or a new module or directory for a
few lines. Copies and twins belong to the reuse probe. Put the smaller
shape in `fix`: what to delete, inline or move. Naming, formatting and taste are not findings.
