---
issue: Closes #1022
raise: crate ai +13, tests +23, comments +2
---
A classifier error names the sidecar's URL once (Closes #1022). When the sidecar was down, every journaled `classify` entry read `http://127.0.0.1:8000/v1/systemone: http://127.0.0.1:8000/v1/systemone: Connection Failed: …` and the pause notice named the sidecar a third time, because ureq's transport error already starts with the URL and the request prefixed it again. A decision error now carries only its cause (`Connection Failed: Connect error: Connection refused (os error 61)`, `the sidecar answered HTTP 401`, `not a decision: …`), and the surfaces that show it — the pause notice, `yi setup`'s probe and `yi doctor` — each name the sidecar's URL once.
