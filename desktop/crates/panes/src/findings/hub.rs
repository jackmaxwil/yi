//! The hub: triage, groups and the finding detail.

use rune::{Cursor, El, Key, KeyEvent, Tx, col, row, spacer, text};
use yd_core::command::by_kind::{FINDING_ASSIGN, FINDING_DISMISS};
use yd_core::command::table::{Run, Target};
use yd_core::feed::Table;
use yd_core::findings::state::Stage;
use yd_core::findings::view::{counts, ready};
use yd_core::ids::{CommandId, PaneId, RowId};
use yd_core::row::{Rows, Tone, Value};
use yd_core::tokens::Tokens;
use yd_ui::kit::bars::{footer, header, lead};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::SEARCH;
use yd_ui::kit::sparkline::sparkline;
use yd_ui::kit::status::{badge, dot, pill};
use yd_ui::kit::toggle::toggle;
use yd_ui::list::state::Lists;
use yd_ui::menu::command_box::{self, Ran};
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{LABEL, META, NONE, colour, white};
use yd_ui::theme::scale::LINE_PICKED;
use yd_ui::win::Win;

use crate::findings::desc::{ACCEPT, ACCEPT_ALL, MERGE, NOT_A_PROBLEM};
use crate::findings::pane::{CHECKS, Form, Item, View, chips, open_count, scope_chip, show};
use crate::findings::{places, ready as gate};

pub mod detail;

/// The groups below the triage box, in the record's order (record 6.6.1).
const GROUPS: [(&str, &[Stage]); 3] = [
    ("Being fixed", &[Stage::Fixing, Stage::Proving]),
    ("Open", &[Stage::Open]),
    ("Being watched", &[Stage::Watching]),
];
/// The right column of the hub, as wide as the mockup's third column.
const SIDE: f32 = 271.0;
/// A finding's two lines (record 6.6.1, as measured).
const ROW_H: f32 = 42.0;
const TITLE: rune::TextStyle = rune::TextStyle {
    weight: 600.0,
    ..LABEL
};

/// Runs a command as a key or a menu would; when nothing carries it out the pane says so.
pub fn press(pane: PaneId, run: Run, title: String) -> impl Fn(&mut Win, &mut Tx) + 'static {
    move |win: &mut Win, tx: &mut Tx| {
        let said = match command_box::run(win, tx, run.clone()) {
            Ran::Done => None,
            Ran::NotBuilt(_) | Ran::Unknown => Some(format!("{title} is not available yet")),
        };
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(view) = hub.panes.get_mut::<View>(Some(pane))
        {
            view.said = said;
        }
    }
}

/// Opens `id` in place, or closes it when it is the one open.
fn toggle_open(win: &mut Win, pane: PaneId, id: &RowId) {
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        if let Some(view) = hub.panes.get_mut::<View>(Some(pane)) {
            view.said = None;
        }
        if let Some(lists) = hub.panes.get_mut::<Lists>(Some(pane)) {
            let list = lists.list_mut("");
            let again = list.open() == Some(id);
            list.set_open((!again).then(|| id.clone()));
        }
    }
}

fn target(item: &Item) -> Target {
    item.finding.address().map_or(Target::App, Target::Thing)
}

/// Accept, Not a problem and Merge: the three answers to a suggestion.
fn answers(pane: &PaneCx<'_>, key: &str, item: &Item) -> Vec<El> {
    let tokens = pane.tokens();
    let id = item.row.id.as_str();
    let made = |command: CommandId, words: &str, kind: Button| {
        let run = Run::new(command).on(target(item));
        button(tokens, kind, words, None)
            .key(format!("{key}.triage.{id}.{}", command.0))
            .on_press_tx(press(pane.pane, run, words.to_owned()))
    };
    vec![
        made(ACCEPT, "Accept", Button::Primary),
        made(NOT_A_PROBLEM, "Not a problem", Button::Plain),
        made(MERGE, "Merge", Button::Ghost),
    ]
}

/// The second line: place, who raised it, who has it, sightings, and the blocking mark.
fn meta(tokens: &Tokens, item: &Item) -> El {
    let said: &[String] = match item.row.get("meta") {
        Some(Value::Tags(said)) => said,
        _ => &[],
    };
    let blocks = item.row.words("blocks");
    let mut parts = Vec::new();
    for (at, words) in said.iter().enumerate() {
        if at > 0 {
            parts.push(text("·").font(META).color(colour(tokens.faint)));
        }
        let ink = if Some(words.as_str()) == blocks {
            tokens.need
        } else {
            tokens.faint
        };
        parts.push(
            text(words.as_str())
                .font(META)
                .color(colour(ink))
                .truncate(),
        );
        if let (1, Some(Value::Series(series))) = (at, item.row.get("seen_series")) {
            parts.push(sparkline(tokens, series));
        }
    }
    row()
        .pl(40.0)
        .gap(7.0)
        .center_y()
        .min_w(0.0)
        .children(parts)
}

/// One finding: its badge, title and state, then its meta line; a press opens it in place.
fn line(pane: &PaneCx<'_>, key: &str, item: &Item, open: bool) -> El {
    let tokens = pane.tokens();
    let id = item.row.id.clone();
    let (severity, level) = match item.row.get("severity") {
        Some(Value::Badge(words, level)) => (words.as_str(), *level),
        _ => ("", Tone::Dim),
    };
    let title = item.row.words("finding").unwrap_or_default();
    let end: Vec<El> = match (item.finding.stage, item.row.get("state")) {
        (Stage::New, _) => answers(pane, key, item),
        (_, Some(Value::Pill(words, role))) => vec![pill(tokens, words, *role)],
        (_, _) => Vec::new(),
    };
    let at = pane.pane;
    let lit = if open { tokens.selected } else { NONE };
    let bar = if open { LINE_PICKED.of(tokens) } else { NONE };
    col()
        .key(yd_ui::list::row::key(key, &id))
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .py(4.0)
        .gap(1.0)
        .fill(colour(lit))
        .border_sides([0.0, 0.0, 0.0, 2.0], colour(bar))
        .cursor(Cursor::Pointer)
        .on_press(move |win: &mut Win| toggle_open(win, at, &id))
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(row().w(32.0).child(badge(tokens, severity, level)))
                .child(
                    text(title)
                        .font(TITLE)
                        .color(colour(tokens.fg))
                        .truncate()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(row().gap(4.0).center_y().children(end)),
        )
        .child(meta(tokens, item))
}

fn finding(pane: &PaneCx<'_>, key: &str, item: &Item, opened: Option<&RowId>) -> El {
    let open = opened == Some(&item.row.id);
    let said = pane.state::<View>().and_then(|view| view.said.as_deref());
    col()
        .w_full()
        .child(line(pane, key, item, open))
        .children(open.then(|| detail::view(pane, key, item, said)))
}

/// The box suggestions wait in until the user files or dismisses them (record 6.6.1).
fn triage(pane: &PaneCx<'_>, key: &str, new: &[&Item], opened: Option<&RowId>) -> El {
    let tokens = pane.tokens();
    let ink = colour(tokens.context);
    let all = Run::new(ACCEPT_ALL).on(Target::Pane(pane.pane));
    let head = row()
        .h(16.0)
        .px(8.0)
        .gap(7.0)
        .center_y()
        .child(
            text(format!("{} new from agents", new.len()))
                .font(TITLE.lh(13.3))
                .color(ink),
        )
        .child(
            text("nothing is filed until you say so")
                .font(META)
                .color(ink),
        )
        .child(spacer())
        .child(
            text("Accept all")
                .font(META)
                .color(ink)
                .key(format!("{key}.accept-all"))
                .cursor(Cursor::Pointer)
                .on_press_tx(press(pane.pane, all, "Accept all".to_owned())),
        );
    col()
        .key(format!("{key}.triage"))
        .mx(4.0)
        .mt(4.0)
        .pt(3.0)
        .pb(2.0)
        .radius(7.0)
        .fill(colour(tokens.context.alpha(0.07)))
        .border(1.0, colour(tokens.context.alpha(0.22)))
        .child(head)
        .children(new.iter().map(|item| finding(pane, key, item, opened)))
}

/// `2 · one agent on both`: the group's own note when its rows carry one, else its count.
fn note(rows: &[&Item]) -> String {
    let carried = rows
        .first()
        .and_then(|item| item.row.words("group.summary"));
    carried.map_or_else(|| rows.len().to_string(), str::to_owned)
}

fn group(pane: &PaneCx<'_>, key: &str, name: &str, rows: &[&Item], opened: Option<&RowId>) -> El {
    let tokens = pane.tokens();
    let first = rows.first().map_or("", |item| item.row.id.as_str());
    let heading = row()
        .key(format!("{key}.group.{first}"))
        .h(20.0)
        .px(8.0)
        .gap(7.0)
        .center_y()
        .child(
            text(name)
                .font(META)
                .text_case(rune::TextCase::Uppercase)
                .color(colour(tokens.faint)),
        )
        .child(text(note(rows)).font(META).color(colour(tokens.dim)));
    col()
        .w_full()
        .child(heading)
        .children(rows.iter().map(|item| finding(pane, key, item, opened)))
}

/// J and K move the open finding; A gives it to an agent; X dismisses it.
fn keys(
    pane: PaneId,
    order: Vec<(RowId, Target)>,
) -> impl Fn(&mut Win, &KeyEvent, &mut Tx) -> bool {
    move |win: &mut Win, event: &KeyEvent, tx: &mut Tx| {
        let Key::Text(typed) = &event.key else {
            return false;
        };
        if event.mods.cmd || event.mods.ctrl || event.mods.alt {
            return false;
        }
        let open = win.hub.try_borrow().ok().and_then(|hub| {
            let lists = hub.panes.get::<Lists>(Some(pane))?;
            lists.list("")?.open().cloned()
        });
        let at = order.iter().position(|(id, _)| Some(id) == open.as_ref());
        let step = |by: isize| {
            let next = at.map_or(0, |at| at.saturating_add_signed(by));
            order.get(next.min(order.len().saturating_sub(1)))
        };
        match typed.as_str() {
            "j" | "k" => {
                let by = if typed == "j" { 1 } else { -1 };
                if let Some((id, _)) = step(by)
                    && Some(id) != open.as_ref()
                {
                    toggle_open(win, pane, id);
                }
            }
            "a" | "x" => {
                let Some((_, aimed)) = at.and_then(|at| order.get(at)) else {
                    return false;
                };
                let command = if typed == "a" {
                    FINDING_ASSIGN
                } else {
                    FINDING_DISMISS
                };
                press(pane, Run::new(command).on(aimed.clone()), typed.to_owned())(win, tx);
            }
            _ => return false,
        }
        true
    }
}

/// The list column: counts, the triage box, the groups by state, the keys.
fn list(pane: &PaneCx<'_>, key: &str, shown: &[&Item]) -> El {
    let tokens = pane.tokens();
    let opened = pane
        .state::<Lists>()
        .and_then(|lists| lists.list(""))
        .and_then(|list| list.open())
        .cloned();
    let opened = opened.as_ref();
    let closed = pane.state::<View>().is_some_and(|view| view.closed);
    let new: Vec<&Item> = shown
        .iter()
        .copied()
        .filter(|item| item.finding.stage == Stage::New)
        .collect();
    let mut body: Vec<El> = Vec::new();
    let mut order = Vec::new();
    if !new.is_empty() {
        body.push(triage(pane, key, &new, opened));
        order.extend(new.iter().copied());
    }
    let groups: &[(&str, &[Stage])] = if closed {
        &[("Closed", &[Stage::Done])]
    } else {
        &GROUPS
    };
    for (name, stages) in groups {
        let rows: Vec<&Item> = shown
            .iter()
            .copied()
            .filter(|item| stages.contains(&item.finding.stage))
            .collect();
        if !rows.is_empty() {
            body.push(group(pane, key, name, &rows, opened));
            order.extend(rows);
        }
    }
    let order = order
        .iter()
        .map(|item| (item.row.id.clone(), target(item)))
        .collect();
    let left = [lead(tokens, &format!("{} open", open_count(shown)))]
        .into_iter()
        .chain(chips(tokens, shown))
        .collect();
    let find = chip(tokens, Chip::Plain, "Find", Some(&SEARCH), false);
    let at = pane.pane;
    let back = chip(tokens, Chip::Plain, "List", None, false)
        .key(format!("{key}.back"))
        .help("Back to the compact list")
        .on_press(move |win: &mut Win| show(win, at, Form::List));
    let done = shown
        .iter()
        .filter(|item| item.finding.stage == Stage::Done);
    let done = done.count();
    let closed_note = (done > 0 && closed).then(|| {
        text(format!("Closed {done} · all proven"))
            .font(META)
            .color(colour(tokens.faint))
    });
    let hints = text("J K move · A assign · X dismiss")
        .font(META)
        .color(colour(tokens.faint));
    col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .focusable(true)
        .on_key(keys(pane.pane, order))
        .child(
            header(tokens, left, vec![back, scope_chip(pane, key), find])
                .key(format!("{key}.pane-header")),
        )
        .child(
            col()
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .key(format!("{key}.rows"))
                .children(body),
        )
        .child(
            footer(tokens, closed_note.into_iter().collect(), vec![hints])
                .key(format!("{key}.footer")),
        )
}

/// A 20 tall title of one part of the right column, its words and its meta.
pub fn title(tokens: &Tokens, key: &str, words: &str, meta: &str) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(20.0)
        .px(tokens.gutter)
        .gap(6.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .child(lead(tokens, words))
        .child(spacer())
        .child(text(meta).font(META).color(colour(tokens.faint)))
}

/// Standing checks: the rules that file findings on their own (record 6.6.5).
fn standing(pane: &PaneCx<'_>, key: &str) -> El {
    let tokens = pane.tokens();
    let empty = Table::default();
    let table = pane.model().tables.get(CHECKS).unwrap_or(&empty);
    let rules: Vec<_> = (0..table.len()).filter_map(|at| table.get(at)).collect();
    let on = |rule: &&&yd_core::row::Row| matches!(rule.get("on"), Some(Value::Flag(true)));
    let lit = rules.iter().filter(on).count();
    let line = |rule: &&yd_core::row::Row| {
        let live = on(&rule);
        let state = if live {
            yd_core::row::Dot::Run
        } else {
            yd_core::row::Dot::Idle
        };
        let id = rule.id.as_str();
        col()
            .key(format!("{key}.rule.{id}"))
            .h(36.0)
            .px(8.0)
            .py(4.0)
            .gap(2.0)
            .opacity(if live { 1.0 } else { 0.5 })
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(dot(tokens, state))
                    .child(
                        text(rule.words("text").unwrap_or_default())
                            .font(TITLE)
                            .color(colour(tokens.fg))
                            .truncate()
                            .grow(1.0)
                            .min_w(0.0),
                    )
                    .child(toggle(tokens, &format!("{key}.rule.{id}.switch"), live)),
            )
            .child(
                text(rule.words("note").unwrap_or_default())
                    .font(META)
                    .color(colour(tokens.faint))
                    .pl(19.0),
            )
    };
    let count = format!("{} checks", rules.len());
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(title(
            tokens,
            &format!("{key}.checks-title"),
            "Standing checks",
            &format!("{lit} on"),
        ))
        .child(
            header(
                tokens,
                vec![
                    lead(tokens, &count),
                    text("they file findings on their own")
                        .font(LABEL)
                        .color(colour(tokens.dim)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.checks.pane-header")),
        )
        .children(rules.iter().map(line))
        .child(rune::spacer())
        .child(
            // The right-click row "Watch for this" is how: the line says the list can grow.
            footer(
                tokens,
                vec![
                    text("Any finding can be turned into a check: \"watch for this\"")
                        .font(META)
                        .color(colour(tokens.faint)),
                ],
                Vec::new(),
            )
            .key(format!("{key}.checks.footer")),
        )
}

/// The page of record 6.6: the list on the left, opened in place, and the gate, the places and
/// the standing checks on the right.
pub fn view(
    pane: &PaneCx<'_>,
    _cx: &mut rune::Cx<Win>,
    key: &str,
    items: &[Item],
    shown: &[&Item],
) -> El {
    let tokens = pane.tokens();
    let all: Vec<_> = items.iter().map(|item| item.finding.clone()).collect();
    let picked = pane.state::<View>().and_then(|view| view.place.as_ref());
    let side = col()
        .w(SIDE)
        .h_full()
        .border_sides([0.0, 0.0, 0.0, 1.0], colour(white(tokens.inner_lines)))
        .child(gate::view(tokens, key, &ready(&all)))
        .child(places::view(pane, key, &counts(&all).places, picked))
        .child(standing(pane, key));
    row()
        .key(key.to_owned())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(list(pane, key, shown))
        .child(side)
}
