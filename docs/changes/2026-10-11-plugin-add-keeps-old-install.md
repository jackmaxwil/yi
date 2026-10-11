---
issue: Closes #1191
raise: crate runtime +32, crate tools +1, comments +1
---
`yi plugin add` and `yi plugin sync` no longer delete a plugin that has its program but not its skills before the replacement arrives. The old install is set aside, the pinned release is downloaded and checked, and only then is the old copy removed; a failed or tampered download puts it back as it was. The comment on the plugin table now says that bumping ripwire's pin also means recording the new release's skills manifest, and the test that reads that manifest finds it by the pinned version. The note that the pier adapter's fingerprint says `+plugins{}` was wrong: pier carries no plugins and its fingerprint has no plugins label.
