//! The Environment column beside a chat: its toggle, its default, its four sections, live.

use std::path::{Path, PathBuf};

use yd_core::ids::RowId;
use yd_core::pane::PaneKind;
use yd_core::settings::schema::SettingValue;
use yd_core::wire::event::WireEvent;
use yd_core::wire::replay::play;
use yd_panes::chat::pane::environment::SETTING;

use super::*;
use crate::start::{self, Files};

/// The key of the chat pane the window draws, found by its bar's environment button.
fn chat(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let button = keys(&shown)
        .into_iter()
        .find(|key| key.starts_with("chat-") && key.ends_with(".pane-environment"))
        .ok_or("no chat bar with an environment button")?;
    Ok(button.trim_end_matches(".pane-environment").to_owned())
}

fn column(runtime: &Runtime<Win>) -> Result<bool, Box<dyn Error>> {
    Ok(live(runtime, &format!("{}.environment", chat(runtime)?)))
}

fn alt_cmd() -> Mods {
    Mods { alt: true, ..cmd() }
}

/// The window over Everything with the column shown by its button.
fn opened() -> Result<(Runtime<Win>, String), Box<dyn Error>> {
    let mut runtime = runtime("shell/everything")?;
    rest(&mut runtime);
    let key = chat(&runtime)?;
    tap(&mut runtime, &format!("{key}.pane-environment"))?;
    Ok((runtime, key))
}

fn said(runtime: &Runtime<Win>, key: &str) -> Result<String, Box<dyn Error>> {
    row_text(&runtime.elements(), key)
}

#[test]
fn the_chat_bars_button_and_its_key_show_and_hide_the_environment_column() -> Check {
    let mut runtime = runtime("shell/everything")?;
    rest(&mut runtime);
    assert!(!column(&runtime)?, "Everything's chat pane is too narrow");
    let key = chat(&runtime)?;
    tap(&mut runtime, &format!("{key}.pane-environment"))?;
    assert!(column(&runtime)?, "the bar's button shows it");
    strike(&mut runtime, Key::Text("e".into()), alt_cmd());
    assert!(!column(&runtime)?, "⌥⌘E hides it");
    let hub = runtime.state.hub.try_borrow()?;
    let kept = hub.model.settings.value(SETTING);
    assert!(
        matches!(kept, Some(SettingValue::Text(said)) if said.as_ref() == "hidden"),
        "{kept:?}"
    );
    Ok(())
}

#[test]
fn a_chat_pane_with_room_shows_the_column_until_the_person_hides_it() -> Check {
    let mut runtime = runtime("chat/environment")?;
    rest(&mut runtime);
    assert!(
        column(&runtime)?,
        "1200 points hold the reading column and the column"
    );
    runtime.set_viewport((900.0, 750.0), 1.0);
    rest(&mut runtime);
    assert!(!column(&runtime)?, "900 do not");
    runtime.set_viewport((1200.0, 750.0), 1.0);
    rest(&mut runtime);
    strike(&mut runtime, Key::Text("e".into()), alt_cmd());
    assert!(!column(&runtime)?, "the person's choice wins over the room");
    Ok(())
}

fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/df16-environment")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("themes"))?;
    Ok(dir)
}

fn started(folder: &Path) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut win = root::init();
    {
        let mut hub = win.hub.try_borrow_mut()?;
        start::files(&mut hub, Files::under(folder));
    }
    let index = scenes::all()
        .iter()
        .position(|scene| scene.name == "chat/environment")
        .ok_or("no scene")?;
    show_scene(&mut win, index);
    let mut runtime = Runtime::new(win, root::view, (1200.0, 750.0), Env::new(true, 1.0));
    runtime.poll = Some(root::poll);
    runtime.on_key = Some(root::on_key);
    rest(&mut runtime);
    Ok(runtime)
}

#[test]
fn hiding_the_column_is_kept_across_a_restart() -> Check {
    let dir = folder("kept")?;
    let mut runtime = started(&dir)?;
    assert!(column(&runtime)?);
    strike(&mut runtime, Key::Text("e".into()), alt_cmd());
    start::quit(&mut runtime.state);
    assert!(!column(&started(&dir)?)?, "still hidden after a restart");
    Ok(())
}

#[test]
fn environment_names_the_folder_branch_model_and_mode_and_folds_by_its_header() -> Check {
    let mut runtime = runtime("chat/environment")?;
    rest(&mut runtime);
    let key = chat(&runtime)?;
    let at = |name: &str| format!("{key}.environment.fact.{name}");
    assert_eq!(said(&runtime, &at("folder"))?, "Folder ~/atlas-api");
    assert_eq!(said(&runtime, &at("branch"))?, "Branch rate-limit-uploads");
    assert_eq!(said(&runtime, &at("mode"))?, "Mode ask");
    assert!(live(&runtime, &format!("{}.copy", at("folder"))));
    tap(&mut runtime, &format!("{key}.environment.environment.head"))?;
    assert!(!live(&runtime, &at("folder")), "folded");
    tap(&mut runtime, &format!("{key}.environment.environment.head"))?;
    assert!(live(&runtime, &at("folder")), "open again");
    Ok(())
}

#[test]
fn context_reads_the_usage_and_compact_asks_yi_to_compact() -> Check {
    let (mut runtime, key) = opened()?;
    let used = said(&runtime, &format!("{key}.environment.context.used"))?;
    assert_eq!(
        used, "21% used 41.2K / 200.0K",
        "usage_update 41200 of 200000"
    );
    let lines = sent(&runtime)?;
    tap(&mut runtime, &format!("{key}.environment.context.compact"))?;
    let slash = {
        let asked = lines.borrow();
        let slash = asked.iter().find(|line| line.contains("\"_yi/slash\""));
        slash
            .cloned()
            .ok_or_else(|| format!("no slash sent: {asked:?}"))?
    };
    assert!(slash.contains("\"line\":\"compact\""), "{slash}");
    // Yi's `_yi/slash` answers `{ text }`; these are its compact verb's words.
    let id = slash
        .split("\"id\":")
        .nth(1)
        .and_then(|rest| rest.split(',').next());
    let reply = format!(
        r#"{{"jsonrpc":"2.0","id":{},"result":{{"text":"compaction scheduled"}}}}"#,
        id.ok_or("no id")?
    );
    runtime.update(|win: &mut Win, _| {
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Wire(WireEvent::Line(reply.clone())));
    });
    rest(&mut runtime);
    let context = said(&runtime, &format!("{key}.environment.context"))?;
    assert!(context.contains("compaction scheduled"), "{context}");
    Ok(())
}

#[test]
fn outputs_list_the_edited_files_newest_first_and_open_one_in_code() -> Check {
    let (mut runtime, key) = opened()?;
    let row = |path: &str| format!("{key}.environment.output.{path}");
    let shown = runtime.elements();
    let upload = by_key(&shown, &row("src/routes/upload.rs")).ok_or("no upload.rs")?;
    let limit = by_key(&shown, &row("src/middleware/rate_limit.rs")).ok_or("no rate_limit")?;
    assert!(upload.rect[1] < limit.rect[1], "the later edit first");
    assert!(
        !live(&runtime, &row("src/routes/upload.rs.read")),
        "a read is no output"
    );
    tap(&mut runtime, &row("src/routes/upload.rs"))?;
    let hub = runtime.state.hub.try_borrow()?;
    let code = hub.model.tables.get(yd_core::feed::OPEN_FILES);
    let opened = code.and_then(|table| table.find(&RowId::new("file:src/routes/upload.rs")));
    assert!(opened.is_some(), "the press opened it in Code");
    Ok(())
}

#[test]
fn background_lists_the_running_children_and_opens_one_in_agents() -> Check {
    let (mut runtime, key) = opened()?;
    let child = format!("{key}.environment.child.agent-1");
    assert!(said(&runtime, &child)?.starts_with("agent 1"));
    tap(&mut runtime, &child)?;
    let hub = runtime.state.hub.try_borrow()?;
    let grid = &hub.model.grid;
    let agents = grid.panes.iter().find(|pane| pane.kind == PaneKind::Agents);
    let agents = agents.ok_or("no Agents pane opened")?.id;
    let lists = hub.panes.get::<yd_ui::list::state::Lists>(Some(agents));
    let open = lists.and_then(|lists| lists.list("").and_then(|list| list.open().cloned()));
    assert_eq!(open.as_ref().map(|id| id.as_str()), Some("agent-1"));
    Ok(())
}

/// More edits than the cap, each a recorded tool event's shape for a new thread.
fn edits(thread: &str, n: usize) -> String {
    let frame = |update: String| {
        format!(
            r#"{{"dir":"recv","frame":{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{thread}","update":{update}}}}}}}"#
        )
    };
    let mut lines = vec![frame(
        r#"{"sessionUpdate":"_yi/replay","from":0,"entries":[]}"#.into(),
    )];
    for i in 0..n {
        let (start, end) = (2 * i, 2 * i + 1);
        lines.push(frame(format!(
            r#"{{"sessionUpdate":"_yi/event","seq":{start},"event":{{"type":"tool_execution_start","toolCallId":"e{i}","toolName":"edit","args":{{"path":"src/f{i}.rs"}}}}}}"#
        )));
        lines.push(frame(format!(
            r#"{{"sessionUpdate":"_yi/event","seq":{end},"event":{{"type":"tool_execution_end","toolCallId":"e{i}","toolName":"edit","result":{{"content":[],"details":{{}}}},"isError":false}}}}"#
        )));
    }
    lines.join("\n")
}

#[test]
fn outputs_past_the_cap_say_what_was_kept_and_where_the_rest_are() -> Check {
    let (mut runtime, key) = opened()?;
    runtime.update(|win: &mut Win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            // A recording carries no times: each line is a second after the one before.
            for (n, line) in edits("many-edits", 9).lines().enumerate() {
                let at = yd_core::ids::Millis(1_794_679_200_000 + 1_000 * n as u64);
                yd_core::msg::step(&mut hub.model, yd_core::msg::Msg::Tick(at));
                let _ = play(&mut hub.model, line);
            }
        }
        yd_ui::shown::show(win, yd_core::ids::ThreadId::new("many-edits"));
    });
    rest(&mut runtime);
    let more = said(&runtime, &format!("{key}.environment.outputs.more"))?;
    assert_eq!(
        more,
        "[…] 8 of 9 files, the newest (cap 8); Changes lists all"
    );
    assert!(
        !live(&runtime, &format!("{key}.environment.output.src/f0.rs")),
        "the oldest cut"
    );
    Ok(())
}

#[test]
fn the_column_follows_the_usage_the_wire_sends() -> Check {
    let (mut runtime, key) = opened()?;
    let thread = {
        let hub = runtime.state.hub.try_borrow()?;
        let shown = yd_ui::shown::thread(&hub, runtime.state.window);
        shown
            .map(|thread| thread.id.as_str().to_owned())
            .ok_or("no thread")?
    };
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{thread}","update":{{"sessionUpdate":"usage_update","used":150000,"size":200000}}}}}}"#
    );
    runtime.update(|win: &mut Win, _| {
        yd_ui::win::dispatch(win, yd_core::msg::Msg::Wire(WireEvent::Line(line.clone())));
    });
    rest(&mut runtime);
    let used = said(&runtime, &format!("{key}.environment.context.used"))?;
    assert_eq!(used, "75% used 150.0K / 200.0K");
    Ok(())
}

#[test]
fn a_shown_column_draws_nothing_while_nothing_changes() -> Check {
    let mut runtime = runtime("chat/empty")?;
    rest(&mut runtime);
    assert!(column(&runtime)?, "a pane the window's width has room");
    let key = chat(&runtime)?;
    let empty = said(&runtime, &format!("{key}.environment.outputs"))?;
    assert!(empty.contains("No thread shown"), "{empty}");
    assert!(!runtime.frame(1.0 / 60.0), "nothing changes, nothing draws");
    Ok(())
}

/// Dogfood: a greyed View menu item kept ⌥⌘E from the window; the key did nothing.
#[test]
fn the_view_menu_lists_show_environment_enabled_with_its_key() -> Check {
    let runtime = runtime("shell/everything")?;
    let hub = runtime.state.hub.try_borrow()?;
    let menus = yd_ui::menu::command_box::menu_bar(&hub);
    let view = menus
        .iter()
        .find(|menu| menu.title == "View")
        .ok_or("no View menu")?;
    let item = view
        .items
        .iter()
        .find(|item| item.title == "Show Environment");
    let item = item.ok_or("no Show Environment in View")?;
    assert_eq!(item.key, Some("alt+cmd+e"));
    assert!(item.enabled, "a greyed item swallows its key on the Mac");
    Ok(())
}
