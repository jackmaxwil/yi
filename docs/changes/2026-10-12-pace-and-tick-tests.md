---
issue: Closes #1248
raise: crate console +85, crate tui +12, comments +4, tests +97
---
Three tests now hold the console's reveal pace and the image-only tick. A recorded `yi console` run must show a streamed reply half-revealed in some frame while a headless one paints it whole, which breaks if `tui.pace` is dropped between the config and a chat. The loop's paint-or-tick step in `yi tui` and `yi console` is now a function (`yi_tui::render::step_frame`, and the console's own), so a test can run a step that draws no text and read that the orb still gets its next frame, inside an update of its own.
