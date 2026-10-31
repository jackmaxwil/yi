---
issue: Closes #1522
raise: crate tools +42, tests +260
---
An edit answered with its whole changed region, every line plus three either side, and never with a line it dropped. A twelve-row edit cost about 1.5x the body it was given, and the guide's own `range sized to new content` mistake, a `PUT N.=M:` whose range runs past the content the body actually replaces, took a live row out of the file with nothing in the reply to show it had happened. The reply is now `updated; +A -R at line N`, the rows the edit removed (`-before N:TEXT`, each clipped to 120 columns, at most 8, the ninth landing a `[… 8 of N removed rows (EDIT_REMOVED_CAP 8); the rest: …]` row naming the read that shows what the file holds now) and the new tag. The rows the model authored stay anchors under that tag without being reprinted, and so do the rows it had already seen, so a follow-up edit on an unprinted line still lands without a re-read.
