---
version: 0.790.0
issue: Closes #1434
raise: crate console +2, comments +2
---
An idle console wakes less often to check its input thread's stop flag (Closes #1434). The thread polled the terminal every 50 ms only so that quitting could stop it; it now checks every 250 ms, and quitting restores the terminal before it joins the thread rather than after, so the longer check never holds the console's screen up. On an idle interactive console under `scripts/tui_pty.py` (debug build, context switches over 20 s), main measured 78, 60 and 67 a second and this change 47, 49 and 51; a 100 ms check measured 56, 64 and 61, too close to main to keep. The socket writer's 200 ms stop check stays: blocking it until its sender dropped closed the socket the moment the console quit, and a reply already in flight then met a broken pipe, which `drive::only_a_sent_prompt_returns_a_scrolled_pane_to_the_bottom` caught ten times in ten.
