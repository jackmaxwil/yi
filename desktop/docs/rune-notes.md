# Rune for Yi Desktop: how to do each job

Read this instead of exploring Rune. Rune is the owner's fork, `https://github.com/jackmaxwil/rune` (branch `yi-desktop`, base `dev@4a2e4379`, 2026-11-18), pinned by revision in `Cargo.toml` (`docs/deps.md`); a Rune patch is a commit to the fork and a new pin. File references are under the fork's `crates/` (`rune/src/…` is the core crate, `rune-kit/src/…` the components). Every fact here was run in a spike (`spikes/`, kept in the staging repository's history) or in this repo unless it says "read, not run". When you learn something new, put it in your report's notes.

## Build and pin

- Toolchain 1.99.1. Rune's five `[patch.crates-io]` entries (`wgpu-hal`, `winit`, `parley_engine`, `accesskit_windows`, `accesskit_macos`) are copied in the root manifest: a dependency's patches do not reach the workspace that uses it.
- Features: `rune` without `ogg-audio` and `dbus`; `rune-kit` with `markdown`. Release builds need `--features tooling` on `yi-desktop` for the headless flags; debug builds always take them.
- Use `cargo fmt`, never `cargo fmt --all` (it formats Rune's vendored crates). Building prints Rune's own warnings; `clippy -D warnings` does not reach path dependencies. There is no `rustfmt.toml`: lines wrap at 100.
- `/usr/bin/python3` has no `tomllib`; the gate needs the python3 that bash finds.

## Open a window

- `rune::run(AppSpec { scenes, min_size, families, poll, ..AppSpec::new(title, (w, h), init, view) })` (`rune/src/tooling.rs:17-60,192`). `init: fn() -> S`, `view: fn(&S, &mut Cx<S>) -> El`, `scenes: &'static [(&'static str, fn(&mut S))]`, `poll: fn(&mut S, f64, &mut Tx) -> bool`, `on_key`, `setup: fn(&mut S, Waker)`. All are `fn` pointers, not closures. This repo's `crates/app/src/root.rs` is the working example.
- `App::new(spec).init(closure)` supplies state from a closure; `AppSpec::new` still needs an `init` fn pointer. `App::keep_running()` keeps the process alive with every window closed. `App::on_ready` runs once natively, and headless only with `--realtime`, after frame 0; its closure gets a `WindowHandle`, not the state: capture `app.handle()` first and reach the state with `handle.update`.
- Each of `App::on_ready`, `on_quit`, `on_close`, `on_focus` and `on_visibility` replaces the hook set before it. A window has one hook of each kind, which calls everyone who needs to hear.
- `rune::is_headless()` reads the process arguments, so it is false inside `cargo test`: pass "native" down as a value. `AppHandle::wake` runs `AppSpec::poll` while the window is hidden, so a main window closed under `keep_running` still drains its inbox.
- Glass window (`spikes/glass/src/main.rs`): `App::new(spec).titlebar(Titlebar::new(25.0)).window_options(WindowOptions { transparent: true, ..Default::default() })`. `Titlebar` gives full-size content, drag regions only, and traffic lights at `(16, round((h-14)/2))` (`rune/src/window/titlebar.rs:64-68`, `create.rs:38-43`). Mark the top bar `.window_drag()`. Read `Cx::chrome_insets().leading` to pad a title past the lights.
- One root `.system_material(SystemMaterial::Glass)`; at most 4 materials a window, 4 `backdrop_blur` a frame. A root with a material does not draw its own fill where the native view exists (`rune/src/runtime/paint/build.rs:899-903`): put the tint in a child. The root's fill is what headless, opaque windows and `RUNE_SYSTEM_MATERIAL=off` draw: use it as the matte value.
- A titled window keeps AppKit's corner radius: `MacWindowOptions::corner_radius` works only for a transparent and borderless window (`rune/src/platform/macos/window_shape.rs:31-33`).
- The view cannot read the window size (`Env` has only `system_dark` and `scale`). Key an element and read `cx.layout_size("key")` (`rune/src/runtime/layout_read.rs:272`); it is right on the first frame and costs about one view run per resize step. `cx.window_layout(key)` gives `[x, y, w, h]`.
- UI zoom: `cx.zoom(k)` on every view run (a run that skips it is unzoomed) lays the view out in `1/k` as many points at `k` times the scale, as a browser's page zoom; layout reads, pointer positions and `chrome_insets` are in zoomed points, and `Runtime::zoom()` reads it in a test. `cx.titlebar_height(h * k)` keeps the traffic lights centred in a zoomed top bar (`rune/src/runtime/zoom.rs`, `cx.rs:213`).
- `rune::extra_args()` returns the flags Rune did not take; `rune::is_headless()` says whether this is a headless run.

## Share state between windows

- Rune gives each window its own state `S`. This app's window state is `yd_ui::win::Win`, which holds `Rc<RefCell<Hub>>`; change the model only through `yd_ui::win::dispatch`. Pattern proven in `spikes/windows/src/win.rs`.
- `AppHandle<S>` is `Send + Sync` for any `S`; `AppHandle::update(closure)` runs the closure on the UI thread and the closure must be `Send`: reach state through the `&mut S` it is given, never by capture (`rune/src/app.rs:36,789`).
- `AppHandle::update(|_, _| {})` is the wake: the view runs again. `AppHandle::update_if(|s, _| .. false)` runs on the UI thread with no redraw. `AppHandle::wake()` makes `poll` run, also while the window is hidden. Waking two other windows costs about 5 µs; each woken window pays one rebuild (0.07 to 0.11 ms).
- More windows: `rune::run_windows(|displays| vec![spec.boxed(), ..])`; `WindowSpec::from_app(key, app)` keeps the `AppHandle`, which has `show`, `hide`, `open`, `close`, `focus`, `set_position` (desktop pixels; AppKit snaps to whole points). `rune::open_window(spec)` at run time returns a `WindowHandle` (`rune/src/window/spec.rs:330`), needed only for `placement()` and `is_ready()`. Ready in 12 to 20 ms.
- A headless `run_windows` without `--desktop` runs only the first window of the factory (or `--window KEY`) as a single-window run, so `--scene`, `--snap` and `--tree` work as with `rune::run`.
- `AppHandle::place(DisplayTarget::Primary, OverlayPlacement::Rect)` moves and sizes a window in points from the main screen's top left, also on the virtual desktop; `set_position` only moves it. `AppHandle::update` runs on a hidden headless window, and hidden headless windows still poll.
- `Titlebar::new(h).traffic_lights(x, y)` places the lights; `Cx::chrome_insets().leading` says where they end; a test sets it with `Runtime::set_chrome_insets`.
- `open_window` does nothing headless (`spec.rs:330-337`): a headless run must return every window it will need from the factory, hidden (`WindowOptions { visible: false }`), and "open" one with `set_position` + `show`. `WindowSpec::on_created` is not called headless. `WindowSpec::options()` resets the Mac window options (`spec.rs:262-266`).
- A window opened while a pointer is held must be `WindowSpec::inactive()`: `Runtime::window_focus(false)` cancels a held press (`rune/src/runtime/frame.rs:84-88`). Focus it on release.
- A window that is not key gets right-click and scroll (read, not run) but no mouse-moved events with the vendored winit, so hover does not follow the pointer there (`vendor/winit/src/platform_impl/macos/view.rs:165-192,629-700`). The fix is an `NSTrackingArea` patch in the fork, scheduled with task WN.
- State that does not travel with a pane: scroll offset, focus, hover, `TabDragState`. Only what is in the hub moves.

## Feed the UI from other threads

- Rune has no async runtime, sockets, processes or PTY. Use `std::thread` and channels. This app's channel is `yd_core::inbox`; the window's `poll` drains it (`yd_ui::win::drain`).
- `poll` returns true only when something visible changed; an idle window draws nothing. For a value that changes every frame use `Live` and `tx.live_only()`.
- Headless order: the scene fn runs first, then `setup(state, Waker)`. Skip live sources in scenes with `rune::is_headless()`. Stop feed threads on `rune::quitting()`.
- Timers: stamp a deadline with `Tx::clock()`, ask `Tx::wake_at(due)`, fire it from `poll` (`rune/src/el/tx.rs:356-369`). Modes are one enum and one `step` (`rune::machine`); timers are events.

## Lay out

- Flex: `col()`, `row()`, `.child(el)`, `.children(iter)` (an `Option<El>` works), `.w(pt)`, `.h(pt)`, `.w_full()`, `.h_full()`, `.grow(share)` (sets basis 0; there is no `basis`), `.min_w(0.0)`, `.min_h(0.0)`, `.gap`, `.px`, `.py`, `.center()`, `.center_y()`, `.justify_end()`, `.self_start()`.
- Absolute: `.absolute(x, y).size(w, h)` (`rune/src/el/layout.rs:562`). For panes use no `.motion()` on the element: a window resize then lands the same frame.
- Hairlines: `.border_sides([top, right, bottom, left], colour)` draws inside the box, takes no layout room and snaps to one device pixel (`rune/src/el/visuals.rs:74`).
- Clip a gliding pane with `.clip_rounded(0.0)` so content laid out at the target size does not spill.
- Layout is solved once at the target: a gliding pane's text takes its final wrap in the commit frame and is never laid out again.
- A `flex_wrap` row needs `w_full().min_w(0.0)` or it takes its content width and never wraps.
- Trap: a text child of `col()` is stretched to the column's width, so its `ElementInfo.rect` is the stretched box. Add `.self_start()` before measuring.

## Draw a list

- `col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0)` around `col().growth(GrowthSpec::default()).virtual_rows(count, row_h, key_fn, build_fn).virtual_rows_version(v)` (`rune/src/el/virtual_rows.rs:80-98`; `spikes/glass/src/list.rs`). `ScrollSpec` has no `Default`.
- `virtual_rows_version` is not optional: without it every commit calls the key closure for every row (100,000 rows: open costs 5.7 ms instead of 0.9 ms). Pass the table's `Rows::version()` plus anything else that changes rows (the open row, slots).
- Build each row as `memo(key, version_value, || ..)` (`rune/src/memo.rs:632`). A memo id is one name in the whole window: a second memo with the same id builds uncached. Prefix it with the pane's key.
- Opening a row in place is just a taller row; rows below move by themselves. Rune measures the row. A slot between lines is one more row with its own key.
- `Tx::scroll_key_to(key, top)` reaches an unbuilt row; `Tx::scroll_to_y`, `Tx::scroll_into_view` exist. A scene is `fn(&mut S)` with no `Tx`: to scroll in a scene, set a field and act on it in `poll`.
- Rune builds about three screens of rows (75 for 25 visible). Measured: 100,000 rows of 20 px scroll at 0.4 to 1.1 ms a frame with zero view runs; about 200 bytes a row.
- Kit `Table` and `Tree` are not used for panes: `Table` rows have one fixed height (`rune-kit/src/table.rs:556`) and both take `fn` pointers that cannot carry a pane id (`table.rs:488`, `tree/view.rs:329`). `FileTree` and `DiffView` (with `annotation` and `on_comment`) are usable as they are.
- Trap: `Runtime::stats.commits` rises on every frame a virtual list mounts rows; it does not mean the view ran. Count view calls yourself.
- The key and build closures of `virtual_rows` get only `&Win`, run after the view has returned, and must be `'static`: they cannot hold the table. Keys are owned `String`s read through the `RefCell`. Call: find the table again from the hub by feed (`tables.get(FeedId)`), as `yd_ui::list` does.
- Rune reads every row key again whenever the row count changes or the version differs. Pass a version that moves only when keys can move: putting the open row in it makes every open cost a full pass. At 100,000 rows an appended row costs about 5.8 ms a frame (release), so a log past that lists a window of its newest rows.
- `format!` in a key function is expensive at scale; `[a, b, c].concat()` halved a 100,000-key pass (3.5 ms for 100,000 keys, 3.0 of it building strings).
- A `memo` inside a virtual row keeps the row's handlers and whatever they captured. Everything a kept row or its handlers read, fn pointers of a description included (they hash), must be in the memo version, or the row keeps the old behaviour.
- `Tx::scroll_into_view` puts a `stick_bottom` viewport at rest (it stops following) and reaches unbuilt rows; `Tx::scroll_to_bottom_instant` pins it again; `ScrollSpec.stick_bottom` may change between view runs; `El::on_at_bottom` fires after first layout and on each change.
- Rows with `on_background_press_tx` inside a viewport that is `focusable(true)` with `on_key`: a press focuses the viewport and keys reach `on_key` before focus navigation. Return false to let a focused button take Return.

## Draw text

- `text(words).font(TextStyle).color(Color)`; `TextStyle::new(size, weight).lh(line_height).family(Family).tracking(points).features("'tnum' 1")` (`rune/src/tok.rs:26-106`). All `const`. This app's four sizes are in `yd_ui::theme::look`.
- Families: `AppSpec { fonts: &[bytes], families: ("System Font", "<mono family>") }` makes plain `text()` the Mac system font and `Family::Mono` the bundled font. `Family::SystemMono` is SF Mono and loads in tests and headless. `Family::Named("…")` names any installed family. A bundled family draws only the weights whose files are bundled (no synthetic bold). Geist Mono is not installed on the build Mac: use `Family::SystemMono` until the font file is supplied.
- Sizes are not rounded: 9.5 pt draws at 9.5 px at 1x. Box width rounds up to 1/64 px, height to the nearest 1/64 px (`rune/src/runtime/commit/text_layout.rs:17`, `text/line_metrics.rs:17-21`).
- System Font digits are proportional by default; `'tnum' 1` makes them tabular. Uppercase is `.text_case(TextCase::Uppercase)` (`rune/src/el/text.rs:677`); `ElementInfo.text` stays the source string.
- One unwrapped line with no ellipsis: `rich(text, spans).truncate_fade_width(0.0)`. `El::single_line()` does nothing on plain or rich text (`el/text.rs:389-402`); `rich()` turns wrapping on. `.truncate()` gives an ellipsis; `.wrap()` wraps.
- Spans: `Span::color`, `size`, `bg`, `bold`, `italic`, `underline` over byte ranges (`rune/src/text/span.rs:52-66`); `El::span_radius`; `TextHighlight` + `El::text_highlights` (`span.rs:316`, `el/text.rs:139`).
- Measure with `rune::text::advance(text, style)` and `rune::text::measure(prefix, spans, style, None)` (`rune/src/text/measure.rs:198,233`). A system mono cell is 0.6182 em (6.4907 pt at 10.5, 5.8726 at 9.5; 12 characters at 10.5 need 78 pt, so the record's 76 pt log time column truncates until Geist Mono is supplied), not the 0.6 em kit `DiffView` assumes; one percent off is a whole character by column 100. A tab is four cells in the system mono face and one space in the built-in sans: draw spaces for it.
- Colours are linear light, not sRGB bytes: `rune::color::hex("#rrggbb")` returns `Option<Color>`, then `.alpha(a)`. This app converts tokens with `yd_ui::theme::look::colour`.
- Markdown: `rune_kit::Markdown::new(src).streaming(true).selectable(true).render(&MarkdownStyle::neutral(true))`, with the style kept in state. `TextArea` has inline pills; `TextEdit::rich()`; IME works there.

## Colour code

- `rune_kit::syntax::{builtin, highlight, Syntax, SyntaxTheme, lang_for_path}` (`rune-kit/src/syntax.rs:125-445`, `syntax/paths.rs:8`). `builtin` is uncached and safe on any thread; `highlight` caches and copies. Both cut a source at 128 KiB: colour longer files in pieces, lexing each new piece again from 48, 192 then 768 lines back until it agrees with the piece before (`spikes/code/src/colour.rs`).
- The lexer gives runs of one of 17 kinds, not tokens: `pub fn ` is one run; `LIMIT`, `self`, `std` and `20` are all `Syntax::Number`; `Some` and `Ok` are `Function`. Find the word under the pointer yourself. The record's palette is mapped onto the 17 kinds.
- A lexer started inside a Python docstring does not recover by itself.
- Squiggles and any stroke: `El::shape(Shape)` on the text element (`rune/src/el/visuals.rs:203`), `Shape::stroke`, `Shape::line` with `LineCap::Butt` (`rune/src/shape.rs:62,89`). `canvas(Shape)` is an element with a shape. There is no public wavy underline.
- The spike's surface API to build on is `spikes/code/API.md`.

## Draw a terminal

- `rune::grid(cols, cell_w, cell_h, Vec<Rc<GridRow>>)`; `GridRow::new(text, spans, style)`, `.fill(start, end, colour)` for backgrounds, selection and the cursor, `.cell(col, width, text, style, colour)` to pin a cell, `.wraps(bool)`; `El::grid_caret(col, row)` (`rune/src/el/grid.rs:73-161`). Keep a row's `Rc` while it is unchanged.
- `cell_w` must equal the font's advance (measure it). Pin every non-ASCII cell with `GridRow::cell`, or a fallback font shifts every later column. The grid has no selection or copy of its own: map `El::on_drag` to cells and draw fills.
- `vt100` 0.15 panics in debug once scrollback passes one screen (`grid.rs:125`): the app keeps its own scrollback ring. Working code: `spikes/term/src/`.
- A GUI-launched app has no `LANG`: set `LANG=en_US.UTF-8` for the shell.

## Keys and pointer

- Press: `.on_press_tx(|s: &mut S, tx: &mut Tx| ..)`; right-click `.on_secondary_press`; a row that is not a button: `.on_background_press_tx` (`rune/src/el/handlers.rs:31`); hover `.on_hover_change`, `.hover(colour)`; pointer inside an element `.on_pointer_move_local` (`el/layout.rs:657`); drag `.on_drag` (phases Down, Move, Up; element-local x, y; `dx`, `dy` are pointer travel even when the window moves); wheel `.on_wheel`; cursor `.cursor(Cursor::..)` (`el/layout.rs:589`).
- Keys: `.focusable(true)` + `.on_key` sees every key before focus navigation, Tab included (`rune/src/runtime/input/keys.rs`). `El::autofocus` works only inside a modal: call `Tx::focus(key, false)` from a handler or use `focus_when_shown`. `AppSpec.on_key` gets keys nothing handled.
- Modifiers: `Tx::mods().cmd` (`rune/src/el/tx/keys.rs:35`). Ctrl chords arrive as `Key::Text("c")` with `mods.ctrl`; Option-A arrives as `Key::Text("å")` with `mods.alt` (use `Tx::key_base()`). No key-up is sent.
- Clipboard: `Tx::copy(text)`. `⌘C` and `⌘A` reach `on_key` first and become edit commands only if it returns false. `⌘V` must return false so the root `El::on_paste` hears it.
- Selection across rows: `El::selectable`, `El::selection_group`, `El::selection_copy` (`rune/src/el/text.rs:483-523`). Rune's own copy joins drawn texts and drops empty lines: set `selection_copy` and copy from the source (share it through an `Rc`). A selectable text takes a drag only inside its own box: give it `.grow(1.0)`. Copy stops without a sign at 10,000 unbuilt rows or 4 MiB (`rune/src/runtime/virtual/selection.rs:22-26`).
- Find what is under a point: `Tx::element_at` gives `Picked`; read its key with `picked.key()` (none for an unkeyed leaf) and `picked.path()`, its keyed ancestors joined by `/` (a key that itself holds `/`, such as a file row's, is split across parts). It finds the topmost element, an `inert()` layer included: `Tx::element_at_in(key, x, y, f)` looks inside one subtree and sees through layers drawn over it, and answers `None` when the scope is missing. A query made from inside a pick's callback waits for a frame nobody asked for: make both queries at once. Every such read runs the view again.
- A right-click: `El::on_secondary_press_local` on an ancestor hears a press on any keyed child that has no handler of its own; `Runtime::secondary_down` and `secondary_up` drive one in process.
- `El::inert()` on a layer marks its descendants inert (no handlers, no focus); layout reads still see them. `on_pointer_move_local` on an ancestor hears moves over its children without taking their hover or presses.
- A `Cx::window_layout` read first made in a later view run answers one run late, and the view is not run again for it: make every layout read on every run, found or not. `cx.layout_size(key)` on a full-window absolute scrim gives the window size on the first frame.
- `El::hover_delays(enter, leave)` with `on_hover_change` is ready-made hover intent; in process, drive it with `pointer_move` and `frame(dt)`.
- A text field keeps its focus and caret in the `TextEdit` the view draws. Draw the same one `on_edit` returns; a copy drawn from elsewhere loses the focus on the next frame and typing reaches nothing. `El::drawn_rect_into`, `El::on_layout_tx` (fires after layout, so a size change lands a frame later), `El::on_screen_layout` (works on the virtual desktop).
- Not in Rune: in-app drag payloads and drop targets, mouse long-press, a shortcut registry, a command palette.

## Tables of fn pointers

- Menu items, scenes and kit accessors take `fn` pointers. Give each row of a table its own function from a pool: `fn slot<const I: usize>(s: &mut Win, tx: &mut Tx) { run(TABLE.get(I)) }` and an array `[slot::<0>, slot::<1>, ..]`. `yd_ui::slots!(slot)` writes the 256-entry array for any signature; `yd_ui::slots::SCENE_SLOTS` is the scene pool. Proven for menus, status rows and several tab strips in one window (`spikes/windows/src/commands.rs`, `view.rs`).

## Menus and the Mac

- `App::menu_item(MenuItem::new(title, fn).id(name).menu("File").key_equivalent("cmd+n".parse::<Shortcut>()?))`, with `.enabled(bool)` to grey one and `.keys_first()` so the window's own handlers get the key first; `App::standard_menus`; `AppHandle::set_menu`. Menus are flat (no submenus). An `App` with no items leaves the bar to the `App` that has them (`rune/src/native_menu/mod.rs:232-236`), so the table's items live on the hub window alone. A headless script fires one with `{"t": 6.2, "menu": "<item id>"}`.
- Native context menu: `Tx::popup_menu(PopupMenu, anchor, on_pick)`.
- Status items: `yd_mac::StatusItem::new(on_pick)` inside `AppHandle::update` from `on_ready` (main thread only); `set(title, &[Row])`, `title()`, `pick_row(index)`. `on_pick` runs outside a Rune turn: hop back with `AppHandle::update`. The value is not `Send`; keep it in the hub window's state.
- Notifications: `rune_notify::{post, remove, on_click}`, from a bundled, signed `.app` only, always silent. Dock badge: `WindowHandle::set_taskbar`.
- In `crates/mac`, `har.py` wants `// SAFETY:` directly above the line holding `unsafe {`; keep each unsafe statement short so rustfmt does not move it.

## Pop-ups inside a window

- Through `rune_kit::root(base, layer)`: `Menu` and `context_menu` (Item, Separator, Label), `Select`, `Combobox`, `PopupList`, `Popover`, `Tooltip`, `HoverCard`, `Dialog`. `HoverCard` has no hover-intent timing: the app owns open and close (0.35 s to start, 0.12 s to end worked). Nothing crosses its window's edge.
- Kit components take a `Kit`: `Kit::neutral(dark)` needs no theme; `Kit::from_palette` builds one from colours.
- `PopupList::build` pushes its own anchored surface and a scrim with `on_close`; option keys are `<list key>-option-<id>`. `build_list` is `pub(crate)`, so a field above the list is its own popover surface.
- An element anchored to a zero-height keyed element pinned inside a block (`pin(Some(-10), Some(17), Some(-10), None)`) places a pop-over at exact offsets; added last at the root it stays above later blocks. Absolute children at negative x draw outside their parent, but a scroll viewport clips them: make the viewport span the window and centre the column inside it.

## Components and icons

- Rune's `press_scale` takes effect only on an element that captures the press: it needs an `on_press` handler, so a component that sets it and leaves the handler to its caller shows no press-in until the caller adds one. `ElementInfo.scale` includes `press_scale` while held.
- `ElementInfo` has no font size or weight. To test a text style, compare the rect width with `rune::text::advance(text, style)` (measure the uppercased string for `text_case`) and the height with the style's line height. `ElementInfo.opacity` includes ancestors; two elements can share one source text (a column title and a fact label), so look up by key.
- Icons: `rune::Icon { name, view, body }` parses SVG arcs and inherits paint from a group. `yd_ui::line_icon!(name, body)` wraps a body in the record's stroke rule (24 box, 1.7, round caps and joins, `currentColor`); `kit::icons::glyph(&ICON, size, ink)` draws one. Declare icon statics with the macro.
- Entry points to reuse: `theme::look::{colour, white, black, tone, kit, CODE, CODE_LINE, READ_*}`, `theme::scale::{Num, Ink, RADIUS_*, load, YI_DARK}`, `kit::status::{dot, thread_dot, dot_colour, tag, badge, pill, swatch}`, `kit::meter::{meter, meter_colour, bar}`, `kit::bars::{header, footer, lead}`, `kit::label::{section, key_values}`. The text shadow under every text, file and context chips and the search box are not built.
- `cargo build` prints Rune's and the vendored crates' warnings; `clippy -D warnings` does not reach them.

## Motion

- Glide a layout change from the handler that makes it: `tx.motion_curve(Curve::tween(0.52, Easing::cubic(0.22, 1.0, 0.36, 1.0)))` (`rune/src/el/tx.rs:694`, `rune-motion/src/curve.rs:23`, `easing.rs:41`). This is the mockup's own curve: at rest after 0.52 s. `Spring::new(0.52, 0.0)` takes 1.27 s and draws all that time.
- It works from `AppHandle::update` and `Runtime::update` too. Colour changes blend with `Tx::color_motion`.
- `Spring::new` asserts its arguments: make springs `const` so a bad value fails the build.
- Also there: `Transition`, `Live::smooth`, `raise`, scale, rotate, `matched(id)` inside one window, reduced motion. Nothing moves on its own in this app.

## Theme values

- `Theme::from_json` is a flat map whose `color` and `num` panic on a missing value (`rune/src/theme.rs:167-189`): use `try_color`. This app's values are `yd_core::tokens::Tokens`; Rune's theme is only what kit components need.
- No file watcher in Rune (rune-luau has one for scripts). `rune::prefs::Prefs` is a ready JSON settings store (`rune/src/prefs.rs:63-100`). Bound every cache.

## Surfaces

- `Pixels::set(w, h, rgba)` from any thread wakes only the windows showing it and never runs the view; draw with `el().pixels_fit(&pixels, SurfaceFit::Fit)`. It asserts `rgba.len() == w*h*4`: build the buffer from the same constants. A 30 Hz picture costs 0.2 ms a frame.
- `NativeSurface::push_bytes(ByteFrame)` (BGRA or NV12) with children drawn on top (`rune/src/native_surface/mod.rs:241`).
- `ImageCache::load(bytes)` decodes PNG, JPEG, GIF, WebP and SVG bytes on a worker and returns an `Image` (`rune/src/image.rs:196-214`).

## Tabs and tear-out

- `rune_kit::shell::TabStrip` with `TabDragState`. Several strips in one window: `drag_state(&state, access)` takes `fn(&mut S) -> &mut TabDragState`; give each strip a slot (`fn strip_state<const I: usize>`).
- Tear-out: `TearOutSpec { lone: true, .. }` for a one-tab strip; on the first `on_tear_move` with `torn == true` open the window inactive and call `TabDragState::hand_off()`; keep listing the tab until `on_tear_out` (`rune-kit/src/shell/tab.rs:203`). The drag never moves to the new window: the source strip keeps the gesture and the app moves the window with `set_position`. A move with `torn == false` means it came back.
- Dock by drag: `TabDragState::set_incoming(Some([x, y]))` in the target window's points; `incoming_slot()` is `Some` only after the target strip has built again. Record each strip's rect with `El::on_screen_layout`.
- `Drag::screen_point()` and `TabTearMove.screen_point` are `None` headless: derive `(window origin + window point) * scale`; `rune::tooling::headless_run().scale()` gives the scale.

## Scripts

- `rune_luau::Script::with_options(name, Sources, ScriptOptions)`, `script.view(cx)`, `reload(sources)` keeps state, `watch(dir)`; sandbox, 250 ms watchdog, 64 MB heap; `HostNode` for app-made pieces, `Host::async_function`. Built-ins: row, col, box, text, spacer, virtualList; press and hover only. The crate is new and has no tests of its own.

## Headless scenes, trees and tests

- `--tree` writes an object `{t, anim_t, viewport, scale, dark_system, elements: [{path, role, rect, fill, key?, text?, label?, help?, opacity, exiting, …}], warnings}`, not a bare array. `--snap X --tree Y` in one run writes both, but the tree write fails (exit 2) unless Y's folder exists; `--snap` alone creates its folder.
- `--script` with `--snap` or `--tree` takes the final state; `{"type": "mark", "name": "end"}` ends it. `{"type": "tap", "key": "<element key>"}` taps by key; key names are capitalised (`Up`, `Escape`, `Enter`); a `resize` event keeps the picture 2400 wide.
- In a tree, an absolute child inside `row()` keeps its place in tree order but draws at its own x: sort by x to read a row (`row_text` does).
- `target/debug/yi-desktop --list-scenes`; `--scene NAME --snap out.png` (2400 x 1500: scale 2; `--scale 1` halves it; the snap creates its folder); `--scene NAME --tree out.json` (JSON whatever the name); `--script file.json` with events `move`, `tap`, `drag` (dx, dy, dur), `scroll`, `{"hold": "cmd"}`, `{"menu": id}`; `--window KEY` picks a window; `--at mark:NAME`. An unknown scene exits 2. Needs a GPU. About 0.4 s a picture in debug.
- Many windows: `--desktop BACKDROP.png --script S.json --window KEY --tree T.json` runs every window with real `AppHandle` updates; the tree gains `windows: [{key, frame, visible, focused}]`. `--realtime` is ignored with `--desktop`. A press goes to the topmost window under the point.
- Headless never shows a material: glass cannot be judged from a snap.
- The virtual desktop runs inside a unit test: `rune::try_run_windows_with_args(RunOptions::default(), ["--desktop", png, "--script", json, "--size", "2400x750", "--snap", out], factory)` returns when the run ends, and `rune::tooling::headless_run().window_frames()` gives each window's frame, visible and focused by key. One run a process at a time; a script's `displays` array makes several screens; a missing script key exits the process.
- A test that changes the hub outside a handler does it inside `runtime.update(..)`, or `frame()` returns false and the view never runs again. `Runtime::on_key` is the field that stands for `AppSpec.on_key`. A tree keeps exiting elements for a frame: filter on `exiting` in text checks.
- A headless `--realtime` run keeps the last drawn pixels for `--snap`: a change that arrives only by wake and poll updates the tree but draws no frame. A pointer move just before the end mark fixes it. A script `tap` whose key is missing ends the run.
- In-process tests need no GPU and run under `cargo test`: `Runtime::new(state, view, (w, h), Env::new(true, 1.0))` (`rune/src/runtime.rs:895`), or `Runtime::with_text(.., TextSystem::new(FONTS, FAMILIES))` for the app's fonts. Then `rt.frame(dt)` (advances the clock; returns whether more frames are wanted), `rt.update(|s, tx| ..)`, `rt.elements()` (path, key, text, rect, fill), `rt.pointer_move/down/up`, `rt.key(KeyEvent)`, `rt.physical_input(PhysicalInput::Modifiers(..))`, `rt.wheel_xy(x, y, dx, dy)` (plain `wheel` scrolls only scroll views), `rt.set_viewport(size, scale)`, `rt.take_titlebar_action()`, `rt.cursor()`, `rt.use_clipboard(rune::clipboard::memory())`, `rt.poll = Some(f)`, `rt.state` (public), `rt.stats.layout_nodes`.
- Budgets: `RUNE_COMMIT_BUDGET_MS=8` prints a line for a commit over budget, at most one a second, in process too.
- On a locked screen or sleeping display, windows are created and moved normally but no frame is drawn and Rune prints `display link creation failed: -6661`. Native frame timing needs an awake display. `RUNE_OPEN_INACTIVE=1` launches without taking the keyboard.

## Traps, collected

- `App::args(vec![])` on a window makes headless flags skip that window: `--snap` then opens it natively and never exits.
- `Runtime::wheel` never reaches `El::on_wheel`; use `wheel_xy`.
- `border_sides` takes four widths, not bools.
- A dispatch made inside the effect sink loses its own effects (the sink is taken out while it runs): hop through `AppHandle::update`.
- `--tree` and `ElementInfo.text` give the source text, not what `text_case` drew.
- A drag that begins in the empty part of a row selects nothing unless the text grows to fill the row.
- A resize of a terminal or any `on_layout` reader lands one or two frames after the layout; run frames before reading.
- `Pixels` and Rune's theme readers panic on bad input; check before calling.
- `rune::grid` and kit `DiffView` assume a cell width; measure the face.
- Layout motion from a view does nothing: ask for it in the handler's `Tx`.

## What Rune does not have (the app builds it)

Built already, so reuse them: the shared store (`Hub`), the pane grid maths (`yd_core::grid`), the open-in-place list (`yd_ui::list`), the kit, the golden and tree helpers (`crates/ui/tests/support/tree.rs`). Still to build:

In-app drag and drop; a code viewer; a PTY and escape-code parser; the feed and input side of a screen surface; hover intent; a free-form right-click menu by kind; a file watcher for themes; slider, chip and number field; menu-bar status items (in `crates/mac`); notarisation and updates (`scripts/mac-app` builds and signs a minimal `.app` only).

## Numbers to design against (M4 Max, release)

| Thing | Cost |
|---|---|
| Layout change with eight content panes | 1.6 ms commit (3 ms first time); 0.7 ms a frame while gliding; budget 8.3 ms |
| 100,000-row list | scroll 0.4 to 1.1 ms a frame; open a row 0.9 ms; 46 MB |
| Code surface, 5,006 lines | p95 0.65 ms a frame; 300 KiB coloured in 2.2 ms |
| Terminal | commit p50 0.8 to 6.9 ms; parsing dominates |
| One change with three windows | 0.3 to 0.5 ms |
| Wire | 680 frames a second on one connection; a 16 ms drain batches 54 frames at p50 |
| Debug builds | 10 to 20 times slower; measure in release |

## Yi's server, as the wire client found it

- A permission ask is sent once and never again after a reconnect. Call: keep the ask and answer by its kept id. A child agent's ask carries no child id; `Thread::asker(call)` finds the child by which transcript holds the tool call.
- A prompt the server has received is handed to the session at once and runs even if the client drops; its reply comes later. Call: do not assume a prompt that got no answer never ran; wait for the resumed replay before offering it again.
- Replay comes in parts of at most 512 entries or 8 MiB, never splitting an entry; a part with `from 0` starts a rebuild. `_yi/rewind` broadcasts a whole replay before it answers; its answer's `unsent` is the prompt to put back in the composer.
- A freshly started daemon answers `session/resume` with `configOptions: []` and sends the options later in `_yi/config`. Call: an empty list must not wipe the options known.
- After a mode change Yi stores custom messages with `customType` `fragment` and `display: false` between a prompt and its answer, and sends `_yi/fragment` updates. Call: views hide `display: false`. The faux model's second reply has empty content.
- The unseen count in `session/list` grows only while no client is attached; the app counts changes while no view shows a thread, and `Thread::unseen()` is the sum. A fork attaches the forking client to the new thread. List rows carry `_meta.yi` with `unseen`, `pinnedMs`, `lastEventMs`, `lastState`, all times in Unix epoch milliseconds.
- Beside the socket Yi keeps `<socket>.stopped` (a deliberate stop: never restart) and `<socket>.invocation` (the dead daemon's serve flags, one per line, to reuse on restart).
- `AgentEvent`, `AgentMessage`, `AcpState`, `ChildActivity` and `SettingKind` have no catch-all case in `yi-types`. Call: keep what fails typed decoding as raw JSON and never let it break a fold.
- `_yi/slash` answers `{ text }`; an unknown command comes back as a JSON-RPC error. A tool result settles its `ToolCall` and is not a transcript message. Yi takes images of up to 10,000,000 base64 characters each and stores a prompt as one entry in one frame, so the app's frame cap is 32 MiB.
- Not kept yet: `ChildSpawned.spawned_by`. Image bytes stay in the model; a transcript is capped by message count, not bytes.
- A permission ask goes only to the client that sent the prompt: another attached client, or one that resumes later, sees only the thread's state. The app must prompt for itself to show an approval card.
- Closing a thread cancels all its asks, one the user has answered but the server has not confirmed included; a thread stays `NeedsYou` after its ask is settled until the server sends `state_update`. The bell holds a thread open while any ask of it is in `wire.asks`.
- `_yi/event` updates for a thread the app does not follow never reach its transcript; a `_yi/replay` with entries does, and its `name` overwrites the thread's title. `session/list` rows are read only when the recording holds the matching `send`. `_yi/plan_progress { plan: { running } }` is the only typed source for a turn's title.
- Yi names its stop marker and flags file with `Path::with_extension`: `daemon.sock` gives `daemon.stopped` and `daemon.invocation`. The invocation file holds the serve flags only, one a line.
- `Windows` deserializes through `try_from = Unchecked`, which runs `check()`: a corrupt places file is refused as it is read. `rune::prefs::Prefs` takes any `Serialize` value, so the app crate keeps the grid with no `serde_json`.
- `rune_kit::diff::parse` gives typed hunks and lines and `FileStatus::Added` for `--- /dev/null`. `yd-panes` cannot name `serde_json`, but a `ToolCall`'s `args` parses JSON through `str::parse`.
- Wire tests: sockets live under `target/wc/<pid>-<n>`, and a failed test leaves its socket for the next run to clear. `fixtures/wire/resume.jsonl` has 3 connections and 9 client sends; each later connection opens with `initialize`.

## The grid in core

- Draw with `Windows::place`, never `layout.areas`. A drag is `line_at` on pointer-down, then `GridMsg::Resize { to }` on each move; it is absolute, so the line waits at the minimum until the pointer comes back. Double-click is `GridMsg::Even`; a drop is `Drop` at the `Spot` `zone_at` names.
- A drag's `Phase::Up` is queued: run a frame after `pointer_up` before reading what it did. `tx.animated()` in a move handler glides a keyed element that moved, so a preview read mid-carry is between two zones: rest first.
- A window of one pane is a pulled-out pane; `saved == Some("just-chat")` says the chat-only layout, and any rearrangement sets `saved` to `None`. Report the real window's place with `GridMsg::Moved`; `Window.frame` is where to open it again.
- Docking onto a pane's edge or the window's is `GridMsg::Drop` (see the architecture's grid section). Not built: a new pane in a new window (tab tear-out), the first window's place (record 2.8.1), and z-order.
- Edge values and `Line.after` are in the layout's own columns (saved layouts are written for chat on the right, though the left is the default); only `place`, `line_at` and `Resize` deal in what is drawn. Panes are numbered from 1 (`PaneId(0)` is `SCENE_PANE`); window ids are one past the highest. Save with `serde_json::to_string(&model.grid)`; reading checks the grid, and unknown fields are dropped.
- serde: a field typed `Option<&'static str>` makes `serde_derive` add a `'de: 'static` bound. Call: use a type alias for the `&'static str`.

## The mockup's measured numbers

- Heights: top bar 25, status bar 17, sidebar 196 (the mockup's folded strip, 158, is gone: open or hidden), pane tab bar 20, pane header 22, footer 20, table header 20, list row 20, code line 16 (chat doc 18), log line 16, new-chat top bar 30. Chat column 770 in the chat doc and 680 in new chat. Where the page and the record disagree, the extracted fixture is the page; the owner's decisions still beat both.
- The chat doc frame loads with `pose=hover`, so its extracted text includes the hover pop-over (the steps list).

## Harness traps

- A support module a test binary uses only in part needs `#![allow(dead_code, reason = ...)]`; `har.py` does not check files under `crates/*/tests`, and clippy flags `type_complexity` in test code.
- The journey runner stops its server by pid and fails if anything under `target/journey/<name>` still runs. Run `yi` only under a home inside the lane's `target/`.
- `har.py` splits an `#[allow(...)]`'s contents on commas, so a comma inside `reason = "..."` fails the HAR gate. It counts consecutive comment lines: a doc comment plus an invariant comment under it is three.

## Wave 6, as joined

- `Tx::focus_when_shown(key, false)` gives the keyboard to an element the next build makes; `El::autofocus` alone left the command box's field unfocused when the box was opened by a press while the composer held the keyboard.
- Rune keeps wake deadlines in a sorted list, drops an exact repeat and spends every due one in one poll (`runtime/frame.rs:419`, `motion/timer_wake.rs`): a poll may ask `Tx::wake_at` for the same deadline each time it runs, which is how the themes folder is looked at once a second while nothing draws.
- `AppSpec.poll` runs in a headless scene render too, so anything the poll puts right (a setting that follows state) acts on a pane's scene: gate it on `Stage::App` or build the scene's state so that it agrees.
- A `const` table of `Setting` rows cannot build its `Cow::Borrowed(&[Choice..])` inside a `const fn` call (the temporary has a destructor): give the choices a `const` slice of their own.
- A `memo` version must hold everything a kept row shows: the open row's "not available yet" line is in it, or the press changes nothing on screen.
- An element drawn past its column is clipped and takes no press: the settings pane's controls need about 700 pt, so a narrow pane draws them but they cannot be pressed. `Runtime::set_viewport` widens a test's window.
- A parity string is looked for in the texts joined in tree order: a legend drawn as letter, name, letter, name is not the mockup's one string of names. Put the names first in the tree and place both absolutely (`rune::text::advance` gives the widths).
- `cargo build -p yd-core` and `cargo test -p yd-core` can call a stale library fresh in a lane whose files a checkout replaced: the feed test failed to compile against a library built before it. `cargo test --workspace` (what the gate runs) rebuilt it; `cargo clean -p yd-core` is the heavy way out.
- `Table::apply` digests rows, so a table a pane builds afresh each view run keeps its version while the rows are the same, and the list engine's memo holds.
- An open list row's key element encloses its detail block: read facts from `<pane>.detail.<id>`, and tap a closed row's key to open it.
- `yd-panes` cannot name `yi-types` or `serde_json`: typed accessors live in `yd-core` (`Agent::doing()`, `Agent::flag()`, `Thread::failed()`, `settings::store::value::Json`), and a `ToolCall`'s `args` parse JSON through `str::parse`.
- A thread ignores `_yi/event` until `session/new` is answered; `permission_requested` needs `title` and `description` to link an ask to its call; the event `seq` starts at 0 with no gaps. A recording carries no times: step `Msg::Tick` before each line.
- A child agent's session (`ChildUpdate.session`) is not in `session/list`: opening it by id is what makes the thread known. Yi's cassette worker names its first tool call `c1`, so a journey can name the waiting step `wait-c1`, and a new pane beside Everything's eight is pane 9.
- The worker log is `<socket>.with_extension("worker.log")`, plain stderr lines with no times. A source starts on its first ask: the Logs pane sends one `Refresh` on its first layout.
- `Prefs::at` renames an unparsable file to `<file>.bad` as it loads, keeps unknown keys, and writes on every `set`: it is not for a file written often or from a handler that must not wait.
- `Tokens::values()` widens `f32` to `f64` (0.62 becomes 0.62000000476): narrow it again before showing or comparing. Do not put a dotted token name in a key: `space.row` makes `.row.`, which parity and `list::row::id_of` take for a list row.
- Reducer stamps are enough for a trace: `Message.at/done`, `ToolCall.started/ended/gate`, `Agent.first_seen/last_seen`; `ToolCall::duration()` prefers the server's `details.durationMs`.
- Yi writes a permission ask (`session/request_permission`) and the event that names its tool call (`permission_requested`) from two places, in either order. The reducer links an ask that comes second to the thread's call that waits at its gate (`Wire::gated`); an ask that names no call cannot be answered from the Trace or Agents pane.

## Upstream `dev@4a2e4379`, as joined (DF24)

- Kit `TabStrip` now selects a tab as the button goes down in both orientations, and a strip with neither `on_reorder` nor `tear_out` keeps its tabs still under a drag. The pane tab bar is the app's own (`chrome::tabs`), not `TabStrip`: its 4-point arm (`grid_view::drag::THRESHOLD`), its drop zones and "a click only shows the tab" are unchanged.
- `El::press_on_down()` runs `on_press` as a mouse button goes down (a menu trigger); a key, a reader and a tap still press on the click. It is opt-in and nothing in Rune or the kit sets it, so the Panes button, the bell and every menu still open on release. `Tx::touch_tap()` says a finger pressed; no handler signature changed.
- A scroll view that itself has focus scrolls 40 points on Up and Down and a page on Space (Shift-Space back), but only after the element's `on_key` and `AppSpec.on_key` declined the key: a list viewport whose `on_key` takes the arrows still moves its cursor. Space no longer presses a focused scroll view that has no `on_press`.
- A wheel run latches onto the innermost viewport with room in its direction until the wheel rests 0.5 s or the pointer leaves, and passes on only to a region around it: an open step row's output at its end hands the wheel to the chat. A notched wheel glides only on Windows and Linux; the Mac moves 1:1.
- Text in a scroller and under growing rows lands on whole device pixels with its box. Against renders made before, scrolled chat and IPython text sits one device pixel off (8 scenes); fills and glyph edges moved by at most 3 levels a channel in every other scene.
- Lists: `El::whole_rows` stacks a column's rows on whole points (off by default); `Tx::grow_visible_tail` lets live rows grow and trade places without moving the window; `Tx::scroll_key_to("row/mark", at)` lands an element inside an unbuilt row in one request; `El::on_at_bottom_if` lets an observer decline the rebuild.
- Markdown memoises settled blocks and shapes code blocks a line at a time (`El::by_line`, for static multi-line wrapped text). Colour-only spans no longer enter the shaping key, so recolouring code reuses its shaped layout; `bg`, `weight`, `size`, `italic`, `underline` and mono spans still reshape.
- Buttons, links, checkboxes, radios, switches, tabs, menu items and options are one accessibility leaf: a text inside one that repeats its name is no longer its own element. Find and press such a control by its own name.
- SVG decoding resolves only raster `data:` URLs (no files, no nested SVG); `rune::image::svg_rgba_cover` rasterises an SVG cropped square. `Tx::prepare_text(view, size)` lays a view's text out ahead without mounting it.
- `rune-taffy` now takes `stacker` (and through it `psm`, whose build script compiles assembly, `ar_archive_writer` and `object`) to continue deep layouts on a grown stack.
