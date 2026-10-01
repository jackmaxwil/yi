---
raise: tests +25, crate runtime +2
---
A second `/land` while a push runs is refused at once: `land()` takes the poller before the
lane lock the running push holds, where it used to wait out the push and then race the first
landing's release, which under load started a second landing. The poller test gates the first
push on a marker file instead of sleeping.
