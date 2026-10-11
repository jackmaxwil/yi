---
issue: Closes #1177
raise: crate permission +39, tests +28, comments +1
---
A ripwire command made only of query flags now runs as a read in auto mode instead of in the sandbox. Every command in ripwire's skills spells `--legend=compact`, and neither the bash tool nor the permission check knew that flag, so about 150 of the skills' 174 commands ran contained and were labelled exec. The flag table moved into the permission crate and grew from 38 to 136 flags. Each new flag was checked against ripwire 0.6.5's own help text and run on a scratch repository, which it left byte for byte unchanged. `--quality-delta` stays exec because it deletes a stale sidecar from the working tree; the edit, baseline, note, export, `--html`, `--cache`, server and run-trace flags stay exec as before. `yi gate` needed no change: it and a real bash call both take the permission check's verdict, so both now say `allow` for `ripwire . --hotspots --legend=compact`.
