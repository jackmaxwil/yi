---
issue: Closes #1434
raise: comments +2, crate console +2
---
An idle console wakes less often to check its input thread's stop flag (Closes #1434). The thread polled the terminal every 50 ms only so that quitting could stop it; it now checks every 250 ms, which is the most quitting waits on it. On an idle interactive console under `scripts/tui_pty.py` (debug build, context switches over 20 s), main measured 78, 60 and 67 a second and this change 47, 49 and 51. The socket writer's 200 ms stop check stays: blocking it until its sender dropped closed the socket the moment the console quit, and a reply already in flight then met a broken pipe, which `drive::only_a_sent_prompt_returns_a_scrolled_pane_to_the_bottom` caught ten times in ten.
