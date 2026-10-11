---
issue: Refs #1019
---
The tbv4-weekly workflow takes a `plugins` input when dispatched by hand: `all` (the default, and what the Monday schedule runs), `none`, or a comma list of plugin names, passed to the harbor adapter as `EVAL_PLUGINS`. A dispatch with `plugins=none` is the other arm of the comparison between Yi with and without ripwire on the same six tasks; the two arms' rows already differ by `+plugins{ripwire@0.6.5}` against `+plugins{}` in the fingerprint, and the posted row's note now says `plugins=all` or `plugins=none` too.
