---
issue: Refs #2160
cut: none crates/acp/src/replay.rs
shares: #2205 it edits the model list in acp lib.rs, this branch edits the prompt and steer handlers
shares: #2167 it edits session reload and close tracking in acp lib.rs, this branch edits the prompt and steer handlers
growth: +309 the clipboard reader (macOS osascript, wl-paste, xclip, read off the UI thread under a 5 s deadline), the composer's image markers, the Cmd+V and Ctrl+V path, the image-carrying prompt and steer commands, and the ACP image block that replaces the old drop; weighed for deletion: Command moved out of app.rs into its own file so app.rs shrinks, the message builder lives in yi-types so runtime and acp share it, and no new dependency was added
ledger+: TUI | Cmd+V or Ctrl+V pastes the clipboard image into the message box as an `[Image #N]` marker that is sent with the message and dropped with the marker; the clipboard is read off the UI thread with a 5 s deadline
raise: crate acp +21, crate console +43, crate tui +225, crate types +20, tests +99, comments +4
---
Cmd+V now pastes an image from the clipboard into the message box as an `[Image #N]` marker, and the image is sent with the message. Ctrl+V does the same where the terminal does not pass Cmd+V through, and a paste that carries no text attaches the clipboard image, which is how terminals report Cmd+V of a screenshot. The key is `paste-image` in the keymap. Deleting a marker drops its image. A clipboard with no image says so in a notice, and an image the model cannot read is refused with the reason.

The clipboard is read with `osascript` on macOS and `wl-paste` or `xclip` elsewhere, as PNG.

The console sends the image to the agent as an ACP `image` block. The agent now reads image blocks that pass the image check where it used to drop every one with a note, and still names a malformed image as dropped, with the reason. `_yi/steer` takes a `prompt` list of blocks for images sent mid-turn.
