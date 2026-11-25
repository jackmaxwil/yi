---
issue: Closes #2368
cut: none desktop/crates/panes/src/chat/messages.rs
cut: none desktop/crates/ui/src/list/view.rs
cut: none desktop/tests/budgets/run.py
cut: none desktop/Cargo.toml
cut: none desktop/crates/app/tests/budgets.rs
cut: none desktop/crates/panes/src/chat_doc/view.rs
---
Yi Desktop holds the display's frame rate with its glass on, a long conversation no longer slows every keystroke and every streamed word, and a streaming answer eases in.

Three things were wrong, each measured before it was changed:

- **The glass halved the frame rate.** In a real window on a 120 Hz display each frame took under 3 ms to build, yet the window drew 68 to 88 frames a second with a quarter to a half of them late. Rune presented every frame inside a window-server transaction while a native material was up, so the material and the content would land together; the app's glass covers the window and never moves. The fork now syncs only the frames in which a material view moves (`desktop/docs/rune-patches.md`, fork `1fab9bb1`), and the pin moves to it. Measured after: 115 frames a second while scrolling and streaming, the same picture.
- **A long turn was read over and over.** For every answer in a turn the chat counted the turn's changed files by reading every message of the turn again, each through a JSON round trip, and looked the answer's model up among a hundred choices. On a turn of 160 tool calls a keystroke took 51 ms and a streaming frame 64 ms. A turn's messages are now read once a frame from their typed form (`yd_core::reduce::said`), the files are counted once a turn, a model's name is looked up once a frame, a folded turn reads only its prompt, and a finished answer is kept (`rune::memo`) until its words or the theme change. Measured after: 6 ms and 6 ms.
- **A streaming answer snapped.** The conversation is now Rune's growth stack: a row that grows glides to its height and the pinned tail follows it, and the words of the answer being written fade in. Under reduced motion they are simply there.

The budgets could not see any of this: they measured small conversations in process, and the one long conversation they had was two thousand one-line turns. `just budgets` now judges three more numbers over a 200-message turn with life-sized tool outputs; `just feel` prints every interaction's median, 95th percentile and worst frame over three long conversations; and `just feel-live` drives one in a real window and reads Rune's frame log, which is where the glass was found. The long conversations are checked to be drawn: a first version numbered its events out of order, the reducer dropped them, and an empty thread measured as fast.

Not done here: opening a long thread still builds all of it (140 to 180 ms for sixty Markdown answers), and a 200-message turn's tool rows are built again each frame, which leaves its 95th percentile near the 8.3 ms a frame allows.
