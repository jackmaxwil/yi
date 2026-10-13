---
issue: Closes #1042
raise: crate tui +32, tests +116, comments +3
---
The edit and write cards now show the rows the model read beside the diff (Closes #1042). A card drew only `details.patch`, so each file's `syntax:` verdict and the `[ripwire check]` block, with its caller rows and cap notices, reached the model and never the screen: in a drive on GLM flash the model quoted ripwire's caller check while the card above it showed only the diff. The card keeps those rows from the result text, verbatim and in order, and draws them under the diff, a syntax error in the error colour; a multi-file edit keeps each file's `[path#TAG]` row above its verdict, so an error names its file; a result with no such rows draws as before. A grep apply's `path: syntax:` rows and a new file's write (no patch) are not drawn yet. The TUI and the console share the card; ACP already sends the full result text.
