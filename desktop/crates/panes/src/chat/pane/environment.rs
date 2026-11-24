//! The Environment column beside the conversation: where the thread works, its context, the
//! files it wrote and what runs behind it, each section folded by its header.

use std::borrow::Cow;

use rune::{Cursor, Cx, El, Icon, Tx, col, el, row, text};
use yd_core::command::table::{Command, Route, Run, Target};
use yd_core::feed::{CHANGES, FeedEvent};
use yd_core::grid::layout::ChatSide;
use yd_core::ids::{CommandId, Millis, PaneId, RowId, SettingKey};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::step::commands::SLASH;
use yd_core::reduce::threads::Thread;
use yd_core::reduce::turns::ToolState;
use yd_core::row::{Dot, Row};
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue, Store};
use yd_core::settings::store::SettingsMsg;
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::Tool;
use yd_ui::grid_view::view::{focused, show_pane};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::icons::{DOWN, glyph};
use yd_ui::kit::label::section;
use yd_ui::kit::meter::meter_colour;
use yd_ui::kit::status::dot;
use yd_ui::line_icon;
use yd_ui::list::state::Lists;
use yd_ui::pane::{PaneCx, SCENE_PANE};
use yd_ui::theme::look::{LABEL, META, colour, white};
use yd_ui::theme::scale::METER_TRACK;
use yd_ui::win::{Hub, Win, dispatch};

use crate::chat::composer::current;
use crate::chat::messages::{count, line};
use crate::chat::tools::open;
use crate::chat_doc::view::{CHEVRON, WIDTH as READING};
use crate::settings::pane::file;

/// Wide enough for a 56 point label and a branch or a model name, the record's dense rows.
pub const COLUMN: f32 = 240.0;
/// Files an Outputs section lists before its `[…]` row.
pub const MAX_OUTPUTS: usize = 8;

pub const TOGGLE: CommandId = CommandId("chat.environment");
pub const COMMAND: Command = Command {
    id: TOGGLE,
    title: "Show Environment",
    key: Some("alt+cmd+e"),
    menu: Some("View"),
    kinds: &[],
    route: Route::Model,
};

/// Unset, the column shows where the pane has room for it beside the reading column.
pub const SETTING: &str = "chat.environment";
const CHOICES: [Choice; 3] = [
    Choice {
        label: Cow::Borrowed("When there is room"),
        value: Cow::Borrowed("auto"),
    },
    Choice {
        label: Cow::Borrowed("Shown"),
        value: Cow::Borrowed("shown"),
    },
    Choice {
        label: Cow::Borrowed("Hidden"),
        value: Cow::Borrowed("hidden"),
    },
];
pub const ROW: Setting = Setting {
    key: SettingKey::fixed(SETTING),
    group: Cow::Borrowed("Chat"),
    name: Cow::Borrowed("Environment column"),
    description: Cow::Borrowed("The column beside a chat: folder, context, outputs, background."),
    control: Control::Select(Cow::Borrowed(&CHOICES)),
    store: Store::App,
    default: Some(SettingValue::Text(Cow::Borrowed("auto"))),
    value: None,
};

static SIDE: Icon = line_icon!(
    "environment",
    r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M15 4v16"/>"#
);
static COPY: Icon = line_icon!(
    "copy",
    r#"<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h8"/>"#
);

/// Context by kind and connector state are not on Yi's wire yet (issues #2284 and #2285).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Environment,
    Context,
    Outputs,
    Background,
}

impl Section {
    const ALL: [Section; 4] = [
        Self::Environment,
        Self::Context,
        Self::Outputs,
        Self::Background,
    ];

    fn slug(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Context => "context",
            Self::Outputs => "outputs",
            Self::Background => "background",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Environment => "Environment",
            Self::Context => "Context",
            Self::Outputs => "Outputs",
            Self::Background => "Background",
        }
    }
}

/// What one chat pane keeps of its column: the folded sections, and whether it drew it.
#[derive(Debug, Default)]
pub struct Column {
    folded: Vec<Section>,
    /// Heard from layout, so the toggle flips what the person sees when no choice is kept.
    drawn: bool,
    /// When Compact was pressed: Yi's answer is the thread's first note from then on.
    compacted: Option<Millis>,
}

/// The person's own choice, once made: shown or hidden.
fn chosen(hub: &Hub) -> Option<bool> {
    match hub.model.settings.value(SETTING) {
        Some(SettingValue::Text(said)) if said.as_ref() == "shown" => Some(true),
        Some(SettingValue::Text(said)) if said.as_ref() == "hidden" => Some(false),
        _ => None,
    }
}

/// The bar's button: drawn in the accent while the column shows.
pub fn tool(pane: &PaneCx<'_>) -> Tool {
    Tool {
        part: "environment",
        icon: &SIDE,
        label: "Show Environment",
        on: pane.state::<Column>().is_some_and(|column| column.drawn),
        command: TOGGLE,
    }
}

/// Shows or hides the column of the pane aimed at (else the window's focused pane) and
/// keeps the choice in the settings file.
pub fn carry(win: &mut Win, _tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    if command != TOGGLE {
        return false;
    }
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return true;
    };
    // Invariant: a window's grid always names a focused pane; a pane drawn alone is the scene's.
    let pane = match target {
        Target::Pane(pane) => *pane,
        _ => focused(&hub, win.window).unwrap_or(SCENE_PANE),
    };
    let drawn = hub
        .panes
        .get::<Column>(Some(pane))
        .or_else(|| hub.panes.get::<Column>(Some(SCENE_PANE)));
    let next = if drawn.is_some_and(|column| column.drawn) {
        "hidden"
    } else {
        "shown"
    };
    let key = SettingKey::fixed(SETTING);
    let set = SettingsMsg::Set(key.clone(), Some(SettingValue::Text(Cow::Borrowed(next))));
    step(&mut hub.model, Msg::Settings(set));
    // A failed write keeps the choice for this run; the Settings pane says why the file failed.
    let _ = file::keep(&mut hub, &key);
    true
}

fn column_mut(win: &mut Win, pane: PaneId, change: impl FnOnce(&mut Column)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(column) = hub.panes.get_mut::<Column>(Some(pane))
    {
        change(column);
    }
}

/// The pane's body with the column on the side away from the window's chat side.
pub fn beside(pane: &PaneCx<'_>, cx: &mut Cx<Win>, key: &str, body: El, fill: rune::Color) -> El {
    // Invariant: the layout read is made on every run, shown or not.
    let wide = cx
        .layout_size(key)
        .is_some_and(|size| size[0] >= READING + COLUMN);
    let shown = chosen(pane.hub).unwrap_or(wide);
    let id = pane.pane;
    let heard = el()
        .key(format!("{key}.environment-drawn.{shown}"))
        .absolute(0.0, 0.0)
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _: [f32; 4], _: &mut Tx| {
            column_mut(win, id, |column| column.drawn = shown);
        });
    let left = pane.model().grid.side == ChatSide::Left;
    let side = shown.then(|| column(pane, key, left));
    let (first, last) = if left { (None, side) } else { (side, None) };
    row()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .fill(fill)
        .child(heard)
        .children(first)
        .child(body.clip_rounded(0.0))
        .children(last)
}

fn column(pane: &PaneCx<'_>, key: &str, right: bool) -> El {
    let tokens = pane.tokens();
    let thread = crate::chat::pane::shown(pane.hub, pane.pane);
    let folded = pane.state::<Column>().map(|c| c.folded.as_slice());
    let parts = Section::ALL.into_iter().map(|part| {
        let open = !folded.is_some_and(|folded| folded.contains(&part));
        let body = open.then(|| {
            let Some(thread) = thread else {
                return vec![muted(tokens, "No thread shown")];
            };
            let rows = rows(pane, key, part, thread);
            if rows.is_empty() {
                vec![muted(tokens, empty(part))]
            } else {
                rows
            }
        });
        col()
            .w_full()
            .key(format!("{key}.environment.{}", part.slug()))
            .child(head(tokens, key, pane.pane, part, open))
            .children(body.into_iter().flatten())
    });
    let edge = if right {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0, 0.0]
    };
    col()
        .key(format!("{key}.environment"))
        .w(COLUMN)
        .h_full()
        .py(4.0)
        .gap(6.0)
        .border_sides(edge, colour(white(tokens.inner_lines)))
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .children(parts)
}

fn empty(part: Section) -> &'static str {
    match part {
        Section::Environment => "Yi has not said where it works",
        Section::Context => "No usage reported yet",
        Section::Outputs => "No files written yet",
        Section::Background => "Nothing running in the background",
    }
}

fn head(tokens: &Tokens, key: &str, pane: PaneId, part: Section, open: bool) -> El {
    let icon = if open { &DOWN } else { &CHEVRON };
    row()
        .key(format!("{key}.environment.{}.head", part.slug()))
        .label(format!("Fold {}", part.title()))
        .pl(tokens.gutter)
        .center_y()
        .cursor(Cursor::Pointer)
        .child(glyph(icon, 9.0, tokens.faint))
        .child(section(tokens, part.title(), None, None).px(4.0))
        .on_press_tx(move |win: &mut Win, _: &mut Tx| {
            column_mut(win, pane, |column| {
                match column.folded.iter().position(|each| *each == part) {
                    Some(at) => drop(column.folded.remove(at)),
                    None => column.folded.push(part),
                }
            });
        })
}

fn muted(tokens: &Tokens, words: &str) -> El {
    let said = text(words.to_owned())
        .font(META)
        .color(colour(tokens.faint));
    col().px(tokens.gutter).py(3.0).child(said.wrap())
}

fn rows(pane: &PaneCx<'_>, key: &str, part: Section, thread: &Thread) -> Vec<El> {
    match part {
        Section::Environment => environment(pane, key, thread),
        Section::Context => context(pane, key, thread),
        Section::Outputs => outputs(pane, key, thread),
        Section::Background => background(pane, key, thread),
    }
}

/// One fact: a faint label, the value, and a button that copies it.
fn fact(tokens: &Tokens, key: &str, name: &str, value: &str) -> El {
    let copied = value.to_owned();
    let copy = row()
        .key(format!("{key}.copy"))
        .label(format!("Copy {name}"))
        .cursor(Cursor::Pointer)
        .child(glyph(&COPY, 10.0, tokens.faint))
        .on_press_tx(move |_: &mut Win, tx: &mut Tx| tx.copy(copied.clone()));
    row()
        .key(key.to_owned())
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(6.0)
        .center_y()
        .child(
            row()
                .w(56.0)
                .child(text(name.to_owned()).font(META).color(colour(tokens.faint))),
        )
        .child(
            text(line(value, 200))
                .font(LABEL)
                .color(colour(tokens.fg))
                .truncate()
                .min_w(0.0)
                .grow(1.0),
        )
        .child(copy)
}

fn repo<'a>(pane: &PaneCx<'a>) -> Option<&'a Row> {
    pane.model().tables.get(CHANGES)?.find(&RowId::new("repo"))
}

fn environment(pane: &PaneCx<'_>, key: &str, thread: &Thread) -> Vec<El> {
    let tokens = pane.tokens();
    let folder = yd_ui::shown::works_in(pane.hub, thread);
    let repo = repo(pane);
    let lane = thread.workdir.as_ref().and_then(|w| w.lane.as_deref());
    let facts = [
        (
            "Machine",
            repo.and_then(|repo| repo.words("host")).map(str::to_owned),
        ),
        ("Folder", Some(folder.to_owned())),
        (
            "Project",
            (thread.cwd != folder).then(|| thread.cwd.clone()),
        ),
        ("Lane", lane.map(str::to_owned)),
        (
            "Branch",
            repo.and_then(|repo| repo.words("branch"))
                .map(str::to_owned),
        ),
        ("Model", current(thread, "model")),
        ("Effort", current(thread, "thought_level")),
        ("Mode", current(thread, "mode")),
    ];
    let place = |name: &str| format!("{key}.environment.fact.{}", name.to_lowercase());
    let drawn = facts.into_iter().filter_map(|(name, value)| {
        let value: String = value?;
        let drawn = fact(tokens, &place(name), name, &value);
        if name != "Folder" {
            return Some(drawn);
        }
        let opens = |win: &mut Win, tx: &mut Tx| {
            show_pane(win, tx, PaneKind::Files);
        };
        Some(drawn.cursor(Cursor::Pointer).on_background_press_tx(opens))
    });
    drawn.collect()
}

fn context(pane: &PaneCx<'_>, key: &str, thread: &Thread) -> Vec<El> {
    let tokens = pane.tokens();
    let (used, window) = (thread.usage.used.0, thread.usage.window.0);
    let mut out = if window == 0 || thread.unreported() {
        vec![muted(tokens, "No usage reported yet")]
    } else {
        usage(tokens, key, used, window)
    };
    let at = yd_core::settings::yi::rows(&pane.model().wire.settings)
        .into_iter()
        .find(|row| row.key.as_str() == "compaction.at")
        .and_then(|row| match row.value {
            Some(SettingValue::Number(n)) if n >= 1.0 => Some(n as u64),
            _ => None,
        });
    let mut notes: Vec<String> = at
        .map(|at| format!("Compacts at {}", count(at)))
        .into_iter()
        .collect();
    match thread.transcript.compactions {
        0 => {}
        1 => notes.push("Compacted once".to_owned()),
        n => notes.push(format!("Compacted {n} times")),
    }
    out.extend(notes.iter().map(|note| muted(tokens, note)));
    let asked = pane.state::<Column>().and_then(|column| column.compacted);
    let (target, pane) = (thread.address().ok().map(Target::Thing), pane.pane);
    let compact = button(tokens, Button::Plain, "Compact", None)
        .key(format!("{key}.environment.context.compact"))
        .on_press_tx(move |win: &mut Win, _: &mut Tx| {
            let Some(target) = target.clone() else { return };
            let run = Run {
                text: Some("compact".to_owned()),
                ..Run::new(SLASH).on(target)
            };
            let now = win.hub.try_borrow().map(|hub| hub.model.now).ok();
            column_mut(win, pane, |column| column.compacted = now);
            dispatch(win, Msg::Command(Route::Wire, run));
        });
    out.push(row().px(tokens.gutter).child(compact));
    let answer = asked.and_then(|asked| thread.notes.iter().find(|note| note.at >= asked));
    out.extend(answer.map(|note| muted(tokens, &line(&note.text, 200))));
    out
}

/// Used over the window as two numbers and a bar.
fn usage(tokens: &Tokens, key: &str, used: u64, window: u64) -> Vec<El> {
    let ratio = (used as f64 / window as f64).clamp(0.0, 1.0) as f32;
    let words = |said: String, ink| text(said).font(META).color(colour(ink));
    vec![
        row()
            .key(format!("{key}.environment.context.used"))
            .px(tokens.gutter)
            .h(tokens.row)
            .center_y()
            .child(words(format!("{:.0}% used", ratio * 100.0), tokens.fg))
            .child(row().grow(1.0))
            .child(words(
                format!("{} / {}", count(used), count(window)),
                tokens.dim,
            )),
        row().px(tokens.gutter).child(
            el().key(format!("{key}.environment.context.bar"))
                .w_full()
                .h(4.0)
                .radius(2.0)
                .fill(colour(METER_TRACK.of(tokens)))
                .child(
                    el().absolute(0.0, 0.0)
                        .size(ratio * (COLUMN - 2.0 * tokens.gutter), 4.0)
                        .radius(2.0)
                        .fill(colour(meter_colour(tokens, ratio))),
                ),
        ),
    ]
}

/// The mockup's thread with the git feed's branch row, in a pane wide enough for the column.
pub fn scene(hub: &mut Hub) {
    super::scenes::mockup(hub);
    let changes = include_str!("../../changes/mockup.changes.json");
    let rows = yd_core::row::rows_from_json(changes).unwrap_or_default();
    step(&mut hub.model, Msg::Feed(CHANGES, FeedEvent::Reset(rows)));
}

/// The files the thread's own calls wrote or edited, newest first, each once.
pub fn written(thread: &Thread) -> Vec<String> {
    let mut calls: Vec<_> = thread
        .transcript
        .tools
        .values()
        .filter(|call| {
            call.state == ToolState::Done && matches!(call.name.as_str(), "edit" | "write")
        })
        .filter_map(|call| {
            let arg = |name: &str| call.args.get(name).and_then(|v| v.as_str());
            let patched = arg("patch").and_then(|p| Some(p.strip_prefix('[')?.split_once('#')?.0));
            let path = arg("path").or_else(|| arg("file_path")).or(patched)?;
            Some((call.ended.or(call.started), path.to_owned()))
        })
        .collect();
    calls.sort_by_key(|call| std::cmp::Reverse(call.0));
    let mut out: Vec<String> = Vec::new();
    for (_, path) in calls {
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

fn outputs(pane: &PaneCx<'_>, key: &str, thread: &Thread) -> Vec<El> {
    let tokens = pane.tokens();
    let files = written(thread);
    let total = files.len();
    let mut out: Vec<El> = files
        .into_iter()
        .take(MAX_OUTPUTS)
        .map(|path| {
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
            let shown = (folder.to_owned(), name.to_owned());
            row()
                .key(format!("{key}.environment.output.{path}"))
                .label(format!("Open {path}"))
                .w_full()
                .h(tokens.row)
                .px(tokens.gutter)
                .gap(6.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .child(
                    text(shown.1)
                        .font(LABEL)
                        .color(colour(tokens.fg))
                        .truncate(),
                )
                .child(
                    text(shown.0)
                        .font(META)
                        .color(colour(tokens.faint))
                        .truncate()
                        .min_w(0.0),
                )
                .on_background_press_tx(move |win: &mut Win, tx: &mut Tx| open(win, tx, &path))
        })
        .collect();
    if total > MAX_OUTPUTS {
        let said = format!(
            "[…] {MAX_OUTPUTS} of {total} files, the newest (cap {MAX_OUTPUTS}); Changes lists all"
        );
        out.push(
            muted(tokens, &said)
                .key(format!("{key}.environment.outputs.more"))
                .cursor(Cursor::Pointer)
                .on_background_press_tx(|win: &mut Win, tx: &mut Tx| {
                    show_pane(win, tx, PaneKind::Changes);
                }),
        );
    }
    out
}

fn background(pane: &PaneCx<'_>, key: &str, thread: &Thread) -> Vec<El> {
    let tokens = pane.tokens();
    let running = thread
        .agents
        .iter()
        .filter(|agent| agent.update.status.as_str() == "running");
    let mut out: Vec<El> = running
        .map(|agent| {
            let id = agent.id.clone();
            let task = agent.update.task.as_deref().unwrap_or_default();
            row()
                .key(format!("{key}.environment.child.{}", id.as_str()))
                .label(format!("Open {}", agent.update.name))
                .w_full()
                .h(tokens.row)
                .px(tokens.gutter)
                .gap(6.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .child(dot(tokens, Dot::Work))
                .child(
                    text(line(&agent.update.name, 40))
                        .font(LABEL)
                        .color(colour(tokens.fg)),
                )
                .child(
                    text(line(task, 80))
                        .font(META)
                        .color(colour(tokens.faint))
                        .truncate()
                        .min_w(0.0),
                )
                .on_background_press_tx(move |win: &mut Win, tx: &mut Tx| {
                    let Some(agents) = show_pane(win, tx, PaneKind::Agents) else {
                        return;
                    };
                    if let Ok(mut hub) = win.hub.try_borrow_mut()
                        && let Some(lists) = hub.panes.get_mut::<Lists>(Some(agents))
                    {
                        lists.list_mut("").set_open(Some(RowId::new(id.as_str())));
                    }
                })
        })
        .collect();
    if let Some(plan) = &thread.work.plan {
        let doing = plan
            .running
            .as_deref()
            .map(|label| format!(" · {}", line(label, 60)));
        let said = format!(
            "Plan {} of {}{}",
            plan.done,
            plan.total,
            doing.unwrap_or_default()
        );
        out.push(muted(tokens, &said).key(format!("{key}.environment.plan")));
    }
    if let Some((done, all)) = thread.work.todo_count() {
        let said = format!("Todos {done} of {all} done");
        out.push(muted(tokens, &said).key(format!("{key}.environment.todos")));
    }
    out
}
