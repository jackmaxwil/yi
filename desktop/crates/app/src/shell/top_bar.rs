//! The top bar: sidebar toggle, project and thread title with its facts, command box, bell
//! and the Panes button.

use rune::{Cursor, Cx, El, Icon, TextStyle, Tx, el, row, text};
use yd_core::command::table::{OPEN_COMMANDS, TOGGLE_NOTIFICATIONS, TOGGLE_SIDEBAR, Target};
use yd_core::ids::CommandId;
use yd_core::reduce::threads::Thread;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::grid_view::picker::BUTTON;
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::kit::key_hint::key_hint;
use yd_ui::line_icon;
use yd_ui::menu::command_box;
use yd_ui::theme::look::{BODY, LABEL, META, NONE, colour};
use yd_ui::theme::scale::{BOLD, HOVER, LINE_BAR, RADIUS_BUTTON, SEMIBOLD};
use yd_ui::win::{Hub, Win};

use crate::shell::status_bar::{running, turn};
use crate::shell::view::{self, PICK_LAYOUT, current, project, repo};

pub const HEIGHT: f32 = 25.0;
/// Room the traffic lights take when the window reports none: three 10 pt lights, 7 apart.
const LIGHTS: f32 = 44.0;

static SIDE: Icon = line_icon!(
    "side",
    r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M9 4v16"/>"#
);
static BELL: Icon = line_icon!(
    "bell",
    r#"<path d="M6 16.500V11a6 6 0 0 1 12 0v5.500l1.500 2h-15zM10 21h4"/>"#
);
static PANEL: Icon = line_icon!(
    "panel",
    r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M12 4v16M3 12h9"/>"#
);

const TITLE: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..BODY
};
const BADGE: TextStyle = TextStyle::new(8.5, BOLD).lh(12.0);
const ON_NEED: Rgba = Rgba {
    r: 0x15,
    g: 0x12,
    b: 0x0a,
    a: 1.0,
};

fn lights(cx: &mut Cx<Win>) -> f32 {
    (cx.chrome_insets().leading - 8.0).max(LIGHTS)
}

fn bar(tokens: &Tokens, lights: f32) -> El {
    row()
        .key("shell.top-bar")
        .w_full()
        .h(HEIGHT)
        .px(8.0)
        .gap(7.0)
        .center_y()
        .window_drag()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(LINE_BAR.of(tokens)))
        .child(el().w(lights).h(10.0))
}

fn button(key: &str, drawing: &'static Icon, ink: Rgba, command: CommandId, said: &str) -> El {
    row()
        .key(key)
        .label(said)
        .center()
        .cursor(Cursor::Pointer)
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| view::run(win, tx, command, Target::App))
        .child(glyph(drawing, 13.0, ink))
}

/// One button for layouts and panes: it opens the panel and shows while the panel is open.
fn panes(tokens: &Tokens, hub: &Hub, open: bool) -> El {
    let keycap = command_box::keymap(hub).hint(PICK_LAYOUT);
    let help = ["Panes", keycap.as_deref().unwrap_or_default()].join(" ");
    let fill = if open { tokens.selected } else { NONE };
    row()
        .key(BUTTON)
        .label("Panes")
        .help(help.trim_end())
        .h(18.0)
        .px(5.0)
        .gap(4.0)
        .center_y()
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(fill))
        .hover(colour(HOVER.of(tokens)))
        .cursor(Cursor::Pointer)
        .on_press_tx(|win: &mut Win, tx: &mut Tx| view::run(win, tx, PICK_LAYOUT, Target::App))
        .child(glyph(&PANEL, 12.0, tokens.dim))
        .child(text("Panes").font(LABEL).color(colour(tokens.dim)))
}

/// The thread's facts after its title: turn and its time, live agents, lines changed.
fn facts(hub: &Hub, thread: &Thread) -> String {
    let mut parts: Vec<String> = turn(&hub.model, thread).into_iter().collect();
    match running(thread) {
        0 => {}
        1 => parts.push("1 agent".to_owned()),
        many => parts.push(format!("{many} agents")),
    }
    let row = repo(&hub.model, project(&thread.cwd));
    let lines = row.and_then(|row| Some((row.words("added")?, row.words("removed")?)));
    if let Some((added, removed)) = lines {
        parts.push(format!("+{added} \u{2212}{removed}"));
    }
    parts.join(" · ")
}

fn search_box(tokens: &Tokens, hub: &Hub) -> El {
    // The key the map gives the command, so the hint and the shortcut cannot differ.
    let keycap = command_box::keymap(hub).hint(OPEN_COMMANDS);
    let keycap = keycap.unwrap_or_default();
    row()
        .key("shell.command-box")
        .w(250.0)
        .h(18.0)
        .px(7.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(tokens.raised))
        .cursor(Cursor::Pointer)
        .on_press_tx(|win: &mut Win, tx: &mut Tx| view::run(win, tx, OPEN_COMMANDS, Target::App))
        .child(glyph(&SEARCH, 11.0, tokens.faint))
        .child(
            text("Search or run a command")
                .font(BODY)
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(key_hint(tokens, &keycap))
}

fn bell(tokens: &Tokens, count: usize) -> El {
    let badge = (count > 0).then(|| {
        row()
            .key("shell.bell.count")
            .absolute(7.0, -5.0)
            .min_w(12.0)
            .h(12.0)
            .px(2.0)
            .center()
            .radius(6.0)
            .fill(colour(tokens.need))
            .child(text(count.to_string()).font(BADGE).color(colour(ON_NEED)))
    });
    let said = "Notifications";
    button("shell.bell", &BELL, tokens.dim, TOGGLE_NOTIFICATIONS, said).children(badge)
}

pub fn view(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let thread = current(hub, win.window);
    let grips = hub.panes.get::<yd_ui::grid_view::view::Grips>(None);
    let open = grips.is_some_and(|grips| grips.of(win.window).picking);
    let heading = thread.map(|thread| {
        let name = project(&thread.cwd).to_owned();
        let title = thread.title.clone().unwrap_or_default();
        row()
            .key("shell.title")
            .gap(7.0)
            .center_y()
            .min_w(0.0)
            .shrink()
            .child(text(name).font(BODY).color(colour(tokens.dim)))
            .child(text("/").font(BODY).color(colour(tokens.faint)))
            .child(
                text(title)
                    .font(TITLE)
                    .color(colour(tokens.fg))
                    .truncate()
                    .min_w(0.0),
            )
            .child(
                text(facts(hub, thread))
                    .font(META)
                    .color(colour(tokens.faint))
                    .truncate()
                    .min_w(0.0)
                    .shrink(),
            )
    });
    let count = yd_panes::notifications::view::unread(hub);
    bar(tokens, lights(cx))
        .child(button(
            "shell.sidebar-toggle",
            &SIDE,
            tokens.faint,
            TOGGLE_SIDEBAR,
            "Toggle sidebar",
        ))
        .children(heading)
        .child(el().grow(1.0).min_w(0.0))
        .child(search_box(tokens, hub))
        .child(el().grow(1.0).min_w(0.0))
        .child(bell(tokens, count))
        .child(panes(tokens, hub, open))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Env, Runtime, TitlebarAction};
    use yd_core::settings::schema::{Setting, SettingValue};
    use yd_ui::grid_view::picker::BUTTON;
    use yd_ui::win::Win;

    use crate::root;

    const ZOOMS: [f64; 4] = [0.5, 1.0, 1.25, 2.5];
    const CONTROLS: [&str; 4] = [
        "shell.sidebar-toggle",
        "shell.command-box",
        "shell.bell",
        BUTTON,
    ];

    fn window(zoom: f64) -> Result<Runtime<Win>, Box<dyn Error>> {
        let win = root::init();
        let row = Setting {
            value: Some(SettingValue::Number(zoom)),
            ..root::ZOOM
        };
        win.hub.try_borrow_mut()?.model.settings.rows.push(row);
        let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
        runtime.frame(1.0 / 60.0);
        Ok(runtime)
    }

    /// Left and right edge of `key`, and the middle of the top bar's height.
    fn span(runtime: &Runtime<Win>, key: &str) -> Result<(f32, f32, f32), Box<dyn Error>> {
        let shown = runtime.elements();
        let found = shown.iter().find(|e| e.key.as_deref() == Some(key));
        let [x, y, w, h] = found.ok_or_else(|| format!("no {key}"))?.rect;
        Ok((x, x + w, y + h / 2.0))
    }

    fn press(runtime: &mut Runtime<Win>, x: f32, y: f32) -> Option<TitlebarAction> {
        runtime.pointer_move(x, y);
        runtime.pointer_down(x, y);
        let asked = runtime.take_titlebar_action();
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        asked
    }

    #[test]
    fn every_free_part_of_the_top_bar_moves_the_window_stroke_after_stroke()
    -> Result<(), Box<dyn Error>> {
        for zoom in ZOOMS {
            let mut runtime = window(zoom)?;
            let (left, right, y) = span(&runtime, "shell.top-bar")?;
            let mut edges = vec![left];
            for key in CONTROLS {
                let (start, end, _) = span(&runtime, key)?;
                edges.extend([start, end]);
            }
            edges.push(right);
            let gaps: Vec<f32> = edges
                .chunks(2)
                .map(|g| g.iter().sum::<f32>() / 2.0)
                .collect();
            for (n, x) in gaps.into_iter().enumerate() {
                let drag = Some(TitlebarAction::Drag);
                assert_eq!(press(&mut runtime, x, y), drag, "zoom {zoom}, gap {n}");
                // The window followed the pointer, so the next grab is the same window point.
                runtime.set_window_origin((100.0 + n as f32 * 40.0, 80.0));
                let again = press(&mut runtime, x, y);
                assert_eq!(again, drag, "zoom {zoom}, gap {n}: a second stroke");
                let zoomed = Some(TitlebarAction::DoubleClick); // codespell:ignore doubleclick
                assert_eq!(press(&mut runtime, x, y), zoomed, "two presses in place");
            }
            for key in CONTROLS {
                let mut runtime = window(zoom)?;
                let (start, end, y) = span(&runtime, key)?;
                let pressed = press(&mut runtime, (start + end) / 2.0, y);
                assert_eq!(pressed, None, "zoom {zoom}: {key} keeps its press");
            }
        }
        Ok(())
    }
}
