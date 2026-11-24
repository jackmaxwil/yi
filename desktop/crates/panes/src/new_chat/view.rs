//! The new-chat page: a greeting, one big box, the project it works in, the threads waiting
//! on you as cards (record 3.3).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use rune::{
    Cursor, Cx, El, FileDialogOptions, PopupItem, PopupMenu, TextCase, TextStyle, Tx, col, row,
    text,
};
use yd_core::feed::CHANGES;
use yd_core::ids::{FeedId, Millis, RowId, SettingKey, ThreadId, WindowId};
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::reduce::threads::{Status, Thread, Want};
use yd_core::row::{Row, Rows};
use yd_core::settings::schema::SettingValue;
use yd_core::settings::store::{PROJECTS, SettingsMsg};
use yd_core::tokens::Tokens;
use yd_ui::kit::icons::{DOWN, glyph};
use yd_ui::kit::status::thread_dot;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{BODY, LABEL, READ_GREETING, TITLE, colour};
use yd_ui::theme::scale::{LINE_SOFT, SOFT};
use yd_ui::win::{Hub, Win};

use crate::chat::composer::{Composer, Form, Send, drawn};
use crate::chat::messages::line;
use crate::chat_doc::view::{SIDE, WIDTH, top};

const KEY: &str = "new-chat";
/// Thread cards shown: the row of three the record draws.
pub const MAX_CARDS: usize = 3;
const CHIP: TextStyle = TextStyle::new(11.5, 400.0).lh(16.0);
const SECTION: TextStyle = LABEL.tracking(0.63);
const FINDINGS: FeedId = FeedId::new("findings");

/// The project chip: a press lists the recent folders and "Choose a folder…".
pub const PROJECT: &str = "new-chat.project";
/// The menu row that opens the Mac's folder chooser.
pub const CHOOSE: &str = "choose";
/// Recent folders the project menu lists.
pub const MAX_RECENT: usize = 8;

/// The folder the app was started in, when that is not `/`: a launch from a terminal in a
/// project. Kept in the hub under no pane; the app sets it as it starts.
#[derive(Debug, Clone, Default)]
pub struct StartedIn(pub Option<String>);

pub const SCENES: &[Scene] = &[Scene {
    name: "new-chat/mockup",
    stage: Stage::View(page),
    setup: scene::mockup,
}];

/// What a card says a thread wants, and the word its status opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub thread: ThreadId,
    pub title: String,
    pub body: String,
    pub status: String,
    pub dot: yd_core::row::Dot,
}

/// `words` as a sentence: a full stop added unless it already ends one.
fn sentence(words: &str) -> String {
    let words = line(words, 160);
    if words.ends_with(['.', '?', '!']) {
        words
    } else {
        format!("{words}.")
    }
}

/// `now` under a minute, else minutes, hours, days.
fn age(since: Millis, now: Millis) -> String {
    match now.0.saturating_sub(since.0) / 60_000 {
        0 => "now".to_owned(),
        m @ 1..60 => format!("{m}m"),
        m @ 60..1440 => format!("{}h", m / 60),
        m => format!("{}d", m / 1440),
    }
}

/// The threads waiting on you, newest ask first, then those at work, at most [`MAX_CARDS`].
pub fn waiting(model: &Model) -> Vec<Waiting> {
    let wire = &model.wire;
    let title = |thread: &Thread| {
        thread
            .title
            .clone()
            .unwrap_or_else(|| "Untitled".to_owned())
    };
    let mut out: Vec<Waiting> = Vec::new();
    for alert in wire.attention().alerts {
        if out.iter().any(|w| w.thread == alert.thread) {
            continue;
        }
        let Some(thread) = wire.threads.get(&alert.thread) else {
            continue;
        };
        let (body, word) = match &alert.want {
            Want::Approval(_) => (
                format!("{} One press to allow.", sentence(&alert.detail)),
                "needs you",
            ),
            Want::NeedsYou => {
                let asked = thread.transcript.last_words().unwrap_or_default();
                (format!("Asks: {}", line(&asked, 160)), "asks you")
            }
            Want::Child(_) => (sentence(&alert.detail), "needs you"),
            Want::Failed | Want::Done => continue,
        };
        out.push(Waiting {
            thread: alert.thread.clone(),
            title: title(thread),
            body,
            status: format!("{word} · {}", age(alert.since, model.now)),
            dot: yd_core::row::Dot::Need,
        });
    }
    for id in wire.attention().working {
        let Some(thread) = wire.threads.get(&id) else {
            continue;
        };
        if out.iter().any(|w| w.thread == id) || wire.status(&id) != Status::Working {
            continue;
        }
        let doing = thread.transcript.last_words().unwrap_or_default();
        out.push(Waiting {
            thread: id.clone(),
            title: title(thread),
            body: format!("{} Nothing needed.", sentence(&doing)),
            status: format!("working · {}", age(thread.last_activity, model.now)),
            dot: yd_core::row::Dot::Work,
        });
    }
    out.truncate(MAX_CARDS);
    out
}

/// A 24 tall chip: 11.5 dim on the raised fill, with a chevron when it opens a menu.
fn chip(tokens: &Tokens, words: &str, menu: bool) -> El {
    let chip = row()
        .h(24.0)
        .px(9.0)
        .gap(5.0)
        .center_y()
        .radius(5.0)
        .fill(colour(tokens.raised))
        .child(text(words.to_owned()).font(CHIP).color(colour(tokens.dim)))
        .children(menu.then(|| glyph(&DOWN, 10.0, tokens.dim)));
    if menu {
        chip.cursor(Cursor::Pointer)
    } else {
        chip
    }
}

fn section(tokens: &Tokens, words: &str) -> El {
    text(words.to_owned())
        .font(SECTION)
        .text_case(TextCase::Uppercase)
        .color(colour(tokens.faint))
        .mt(6.0)
}

fn card(tokens: &Tokens, waiting: &Waiting) -> El {
    let id = waiting.thread.clone();
    col()
        .key(format!(
            "{KEY}.card.{}",
            waiting.thread.as_str().replace('/', ":")
        ))
        .w_full()
        .min_w(0.0)
        .px(11.0)
        .py(10.0)
        .gap(4.0)
        .radius(11.0)
        .fill(colour(SOFT.of(tokens)))
        .border(1.0, colour(LINE_SOFT.of(tokens)))
        .cursor(Cursor::Pointer)
        .on_press(move |win: &mut Win| yd_ui::shown::show(win, id.clone()))
        .child(
            text(waiting.title.clone())
                .font(TITLE)
                .color(colour(tokens.fg))
                .truncate(),
        )
        .child(
            text(waiting.body.clone())
                .font(BODY.lh(15.5))
                .color(colour(tokens.dim))
                .wrap(),
        )
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(thread_dot(tokens, waiting.dot, 8.0, true))
                .child(
                    text(waiting.status.clone())
                        .font(LABEL)
                        .color(colour(tokens.faint)),
                ),
        )
}

/// The folders a new thread may start in, newest first: the ones the app knows, then those
/// of the threads the server lists; only folders that still exist, at most [`MAX_RECENT`].
pub fn recent(hub: &Hub) -> Vec<String> {
    let known = hub.model.settings.projects().into_iter();
    let listed = hub.model.wire.listed();
    let mut listed: Vec<&Thread> = listed.into_iter().collect();
    listed.sort_by_key(|thread| std::cmp::Reverse(thread.last_activity));
    // A thread's own checkout is not a project: its `cwd` is the project it came from.
    let threads = listed.into_iter().map(|t| t.cwd.clone());
    let mut out: Vec<String> = Vec::new();
    for folder in known.chain(threads) {
        if out.len() >= MAX_RECENT {
            break;
        }
        // Incident: a checkout of Yi's, named "0", was offered as a project and a thread ran in it.
        let lane = folder.contains("/.yi/lanes/");
        if !folder.is_empty() && !lane && !out.contains(&folder) && Path::new(&folder).is_dir() {
            out.push(folder);
        }
    }
    out
}

/// The folder a new thread starts in: the one chosen for it, else the project the window's thread
/// belongs to, else where the app was started, else the most recent project, else home.
pub fn folder(hub: &Hub, window: WindowId) -> Option<String> {
    let chosen = yd_ui::shown::chosen(hub, window).map(str::to_owned);
    let shown = || yd_ui::shown::thread(hub, window).map(|t| t.cwd.clone());
    let started = || hub.panes.get::<StartedIn>(None).and_then(|s| s.0.clone());
    let recent = || recent(hub).into_iter().next();
    let home = || std::env::var("HOME").ok();
    chosen
        .or_else(shown)
        .or_else(started)
        .or_else(recent)
        .or_else(home)
}

/// A folder's name: its last part.
fn name(folder: &str) -> &str {
    folder
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(folder)
}

/// Keeps `folder` first among the known projects, in the settings file.
pub fn record(win: &Win, folder: &str) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let value = hub.model.settings.with_project(folder);
    known(&mut hub, value);
}

/// When the known projects were last looked at, and what they were then.
#[derive(Default)]
struct Looked {
    at: Option<f64>,
    projects: Vec<String>,
}

/// Drops each known project whose folder is gone, so no poll asks for its threads again;
/// looked at once a second, and as soon as the list changes. The poll calls it.
pub fn forget_gone(win: &Win, now: f64) {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let projects = hub.model.settings.projects();
    let Some(looked) = hub.panes.get_mut::<Looked>(None) else {
        return;
    };
    let due = looked.projects != projects || looked.at.is_none_or(|at| now - at >= 1.0);
    if !due {
        return;
    }
    let there = projects.iter().filter(|folder| Path::new(folder).is_dir());
    let there: Vec<String> = there.cloned().collect();
    *looked = Looked {
        at: Some(now),
        projects: there.clone(),
    };
    if there.len() < projects.len() {
        known(&mut hub, yd_core::settings::store::projects_value(&there));
    }
}

/// Sets the known projects, kept in the settings file.
fn known(hub: &mut Hub, value: SettingValue) {
    let key = SettingKey::new(PROJECTS);
    step(
        &mut hub.model,
        Msg::Settings(SettingsMsg::Set(key.clone(), Some(value))),
    );
    if let Err(why) = crate::settings::pane::file::keep(hub, &key) {
        let _ = writeln!(
            std::io::stderr(),
            "yi-desktop: the project was not kept: {why}"
        );
    }
}

/// Shows New chat on `folder`, and keeps it among the known projects.
pub fn pick(win: &mut Win, tx: &mut Tx, folder: String) {
    record(win, &folder);
    yd_ui::shown::choose(win, folder);
    tx.focus_when_shown("new-chat.composer.input", false);
}

/// Opens the Mac's folder chooser; the folder chosen is New chat's.
pub fn choose_folder(win: &mut Win, tx: &mut Tx) {
    let at = win
        .hub
        .try_borrow()
        .ok()
        .and_then(|hub| folder(&hub, win.window));
    let options = FileDialogOptions {
        title: "Choose a Project Folder".to_owned(),
        directory: at.map(PathBuf::from),
        ..FileDialogOptions::default()
    };
    tx.open_folder(options, |win: &mut Win, chosen, tx: &mut Tx| {
        let chosen = chosen.ok().flatten();
        if let Some(folder) = chosen.as_deref().and_then(Path::to_str) {
            pick(win, tx, folder.to_owned());
        }
    });
}

/// The project menu under the chip: the recent folders, then the chooser.
fn projects(win: &mut Win, tx: &mut Tx) {
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let current = folder(&hub, win.window);
    let recent = recent(&hub);
    drop(hub);
    let items = recent.iter().map(|folder| {
        let item = PopupItem::new(folder.clone(), name(folder));
        item.checked(current.as_ref() == Some(folder))
    });
    let mut menu = PopupMenu::new().items(items);
    if !recent.is_empty() {
        menu = menu.separator();
    }
    let menu = menu.item(PopupItem::new(CHOOSE, "Choose a folder…"));
    tx.popup_menu(
        menu,
        PROJECT,
        |win: &mut Win, picked, tx: &mut Tx| match picked {
            Some(CHOOSE) => choose_folder(win, tx),
            Some(folder) => pick(win, tx, folder.to_owned()),
            None => {}
        },
    );
}

/// The branch a new thread starts on: the repository's base, else its branch.
fn branch(model: &Model) -> Option<String> {
    let table = model.tables.get(CHANGES)?;
    let repo: &Row = table.find(&RowId::new("repo"))?;
    let name = repo.words("base").or_else(|| repo.words("branch"))?;
    Some(line(name, 60))
}

/// The page, under its title: what a window shows in place of its shell.
pub fn page(win: &Win, hub: &Hub, _cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let model = &hub.model;
    let pane = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    let folder = folder(hub, win.window).unwrap_or_default();
    let project = name(&folder).to_owned();
    let shown = yd_ui::shown::thread(hub, win.window);
    let composer = Composer {
        key: KEY,
        pane: SCENE_PANE,
        tokens,
        thread: shown,
        draft: drawn(&pane),
        queued: Vec::new(),
        held: false,
        files: None,
        hint: "Describe it, paste an error, or drop a file or screenshot",
        form: Form::Reading { gap: 22.0 },
        send: Send::New(folder.clone()),
    };
    let findings = model.tables.get(FINDINGS).map_or(0, |t| t.len());
    let context = row()
        .w_full()
        .justify_center()
        .gap(6.0)
        .children((!project.is_empty()).then(|| {
            chip(tokens, &project, true)
                .key(PROJECT)
                .label(format!("Project: {project}"))
                .help(folder.clone())
                .on_press_tx(projects)
        }))
        .children(branch(model).map(|b| chip(tokens, &format!("on branch {b}"), false)))
        .children((findings > 0).then(|| {
            let noun = if findings == 1 { "finding" } else { "findings" };
            chip(tokens, &format!("{findings} {noun} open"), false)
        }));
    let cards = waiting(model);
    let column = col()
        .key(format!("{KEY}.chat-column"))
        .w_full()
        .max_w(WIDTH)
        .px(SIDE)
        .gap(14.0)
        .child(
            row().w_full().justify_center().mb(4.0).child(
                text("What are we working on?")
                    .font(READ_GREETING)
                    .color(colour(tokens.fg)),
            ),
        )
        .children(crate::chat::tray::view(&pane))
        .child(composer.view())
        .child(context)
        .children((!cards.is_empty()).then(|| section(tokens, "Pick up where you left off")))
        .children((!cards.is_empty()).then(|| {
            row()
                .w_full()
                .equal_columns(MAX_CARDS)
                .gap(10.0)
                .children(cards.iter().map(|w| card(tokens, w)))
        }));
    col()
        .w_full()
        .h_full()
        .child(top(tokens, "New chat", &project))
        .child(col().w_full().grow(1.0).min_h(0.0).center().child(column))
}

mod scene {
    //! New chat over the mockup's three threads: one asks to run a command, one asks a
    //! question, one is at work.

    use yd_core::feed::{CHANGES, FeedEvent};
    use yd_core::msg::{Msg, step};
    use yd_core::row::rows_from_json;
    use yd_ui::win::Hub;

    use super::FINDINGS;
    use crate::chat_doc::view::scene::{NOW, OPTIONS, Script, assistant, quote, recv, send, words};

    const RATE: &str = "rate-limit-the-upload-endpoint";
    const COLD: &str = "why-is-cold-start-900-ms";
    const MIGRATE: &str = "migrate-sessions-to-uuids";
    const TIDY: &str = "tidy-ci-cache-keys";
    const MINUTE: u64 = 60_000;
    const FOUND: &str = include_str!("../../../../fixtures/mockup/findings.rows.json");
    /// The project's repository: the branch it is on and the base a new thread starts from.
    const REPO: &str = r#"[{"id":"repo","cells":{"branch":{"text":"rate-limit-uploads"},"base":{"text":"main"},"project":{"text":"atlas-api"}}}]"#;

    fn session(id: &str, title: &str, state: &str, last: u64) -> String {
        format!(
            r#"{{"sessionId":"{id}","cwd":"/work/atlas-api","title":{},"_meta":{{"yi":{{"unseen":0,"lastState":"{state}","lastEventMs":{last},"pinnedMs":null}}}}}}"#,
            quote(title)
        )
    }

    /// What the server replays of a thread: its last answer, stored at `at`.
    fn said(hub: &mut Hub, thread: &'static str, at: u64, text: &str) {
        let message = assistant(&words(text), 0, "0");
        let mut s = Script {
            hub,
            thread,
            seq: 0,
        };
        s.at(at).set(&format!(
            r#""sessionUpdate":"_yi/replay","contextWindow":200000,"from":0,"goal":null,"todos":null,"entries":[{{"type":"message","id":"m1","message":{message},"parentId":null,"seq":1,"timestamp":{at}}}]"#
        ));
    }

    pub(super) fn mockup(hub: &mut Hub) {
        let sessions = [
            session(
                RATE,
                "Rate-limit the upload endpoint",
                "requires_action",
                NOW,
            ),
            session(
                COLD,
                "Why is cold start 900 ms?",
                "requires_action",
                NOW - 11 * MINUTE,
            ),
            session(
                MIGRATE,
                "Migrate sessions to UUIDs",
                "running",
                NOW - 3 * MINUTE,
            ),
            session(TIDY, "Tidy CI cache keys", "idle", NOW - 2_880 * MINUTE),
        ]
        .join(",");
        {
            let mut s = Script {
                hub: &mut *hub,
                thread: RATE,
                seq: 0,
            };
            s.at(NOW - 12 * MINUTE)
                .line(&recv(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
                ))
                .line(&send(r#"{"jsonrpc":"2.0","id":2,"method":"session/list","params":{}}"#))
                .line(&recv(&format!(
                    r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessions":[{sessions}]}}}}"#
                )));
        }
        said(hub, COLD, NOW - 11 * MINUTE, "profile the release build?");
        said(hub, MIGRATE, NOW - 3 * MINUTE, "Editing the migration");
        let mut s = Script {
            hub: &mut *hub,
            thread: RATE,
            seq: 0,
        };
        s.at(NOW)
            .set(&format!(
                r#""sessionUpdate":"_yi/config","contextWindow":200000,"configOptions":{OPTIONS}"#
            ))
            .line(&recv(&format!(
                r#"{{"jsonrpc":"2.0","id":"perm_{RATE}_1","method":"session/request_permission","params":{{"sessionId":"{RATE}","title":"bash requires permission","description":"Wants to run cargo test","options":[{{"optionId":"allow_once","name":"Allow once","kind":"allow_once"}},{{"optionId":"reject_once","name":"Reject","kind":"reject_once"}}]}}}}"#
            )));
        let rows = |text: &str| rows_from_json(text).unwrap_or_default();
        step(
            &mut hub.model,
            Msg::Feed(FINDINGS, FeedEvent::Reset(rows(FOUND))),
        );
        for repo in rows(REPO) {
            step(&mut hub.model, Msg::Feed(CHANGES, FeedEvent::Upsert(repo)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Env, Runtime};
    use yd_core::ids::WindowId;
    use yd_ui::win::{Hub, Win};

    use super::{scene, waiting};

    type Check = Result<(), Box<dyn Error>>;

    fn hub() -> Result<yd_ui::win::Shared, Box<dyn Error>> {
        static DEFS: [&yd_ui::pane::PaneDef; 1] = [&crate::chat::DEF];
        let hub = Hub::new(&DEFS, Vec::new());
        scene::mockup(&mut *hub.try_borrow_mut()?);
        Ok(hub)
    }

    #[test]
    fn the_threads_waiting_on_you_come_first_then_one_at_work() -> Check {
        let hub = hub()?;
        let hub = hub.try_borrow()?;
        let cards = waiting(&hub.model);
        let said: Vec<(&str, &str)> = cards
            .iter()
            .map(|c| (c.body.as_str(), c.status.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (
                    "Wants to run cargo test. One press to allow.",
                    "needs you · now"
                ),
                ("Asks: profile the release build?", "asks you · 11m"),
                ("Editing the migration. Nothing needed.", "working · 3m"),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_press_on_a_card_opens_its_thread() -> Check {
        fn root(win: &Win, cx: &mut rune::Cx<Win>) -> rune::El {
            match win.hub.try_borrow() {
                Ok(hub) => super::page(win, &hub, cx),
                Err(_) => rune::col(),
            }
        }
        let mut rt = Runtime::new(
            Win::new(hub()?, WindowId(0)),
            root,
            (1192.0, 712.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.016);
        let key = "new-chat.card.why-is-cold-start-900-ms";
        let tree = rt.elements();
        let card = tree.iter().find(|e| e.key.as_deref() == Some(key));
        let [x, y, w, h] = card.ok_or("no card")?.rect;
        rt.pointer_down(x + w / 2.0, y + h / 2.0);
        rt.pointer_up(x + w / 2.0, y + h / 2.0);
        rt.frame(0.016);
        let hub = rt.state.hub.try_borrow()?;
        let shown = yd_ui::shown::thread(&hub, WindowId(0)).map(|t| t.id.as_str().to_owned());
        assert_eq!(shown.as_deref(), Some("why-is-cold-start-900-ms"));
        Ok(())
    }
}
