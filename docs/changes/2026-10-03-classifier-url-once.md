---
issue: Closes #1022
raise: crate ai +1, tests +18
---
A classifier error names the sidecar's URL once (Closes #1022). When the sidecar was down, every journaled `classify` entry and the classifier's pause notice read `http://127.0.0.1:8000/v1/systemone: http://127.0.0.1:8000/v1/systemone: Connection Failed: …`, because ureq's transport error already carries the URL and the request prefixed it again; it now passes ureq's text through when that names the URL, and prefixes it only when it does not.
