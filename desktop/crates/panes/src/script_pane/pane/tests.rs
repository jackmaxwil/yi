//! What a user of a plugin pane would notice if it broke, drawn in process with no GPU.

use std::cell::RefCell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use rune::{Cx, El, ElementInfo, Env, Runtime, col};
use rune_luau::Data;
use yd_core::command::tools::fetch::GET;
use yd_core::feed::SourceCmd;
use yd_core::ids::WindowId;
use yd_core::msg::Effect;
use yd_core::pane::PaneKind;
use yd_ui::pane::{PaneCx, PaneDef, SCENE_PANE, Stage};
use yd_ui::win::{Hub, Win};

use super::library::{Manifest, Plugin};
use super::scenes::{MANIFEST, NAME, SCENES, URL, answered};
use super::{DEF, build, library, open, tick};

type Check = Result<(), Box<dyn Error>>;
type Sent = Rc<RefCell<Vec<Effect>>>;

const COUNTER: &str = r#"
local pane = require("@yi/pane")
return { view = function(ctx)
    local n = (ctx.store.get("runs") or 0) + 1
    ctx.store.set("runs", n)
    return { pane.stat("Runs", n) }
end }
"#;
const FOREVER: &str = r#"
return { view = function(ctx) while true do end end }
"#;
const ASKS: &str = r#"
local pane = require("@yi/pane")
return { view = function(ctx)
    local q = ctx.http.get("http://ci-ubuntu:9100/queue")
    ctx.http.get("http://elsewhere:80/")
    return { pane.stat("Waiting", if q then q.waiting else "-") }
end }
"#;

fn root(win: &Win, cx: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    (DEF.view)(&pane, cx)
}

/// The pane alone, with every effect the window hands on kept in `sent`.
fn draw(setup: impl FnOnce(&mut Hub)) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    static DEFS: [&PaneDef; 1] = [&DEF];
    let hub = Hub::new(&DEFS, Vec::new());
    let sent: Sent = Rc::default();
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = Stage::Pane(PaneKind::Script);
        let kept = sent.clone();
        hub.set_sink(move |effect| kept.borrow_mut().push(effect));
        setup(&mut hub);
    }
    let rt = Runtime::new(
        Win::new(hub, WindowId(0)),
        root,
        (316.0, 388.0),
        Env::new(true, 1.0),
    );
    Ok((rt, sent))
}

fn at(rt: &mut Runtime<Win>, now: f64) {
    rt.update(|win, tx| {
        tick(win, now, tx);
    });
    for _ in 0..3 {
        rt.frame(0.016);
    }
}

fn texts(rt: &Runtime<Win>) -> Vec<String> {
    let tree = rt.elements();
    let live = tree.iter().filter(|e: &&ElementInfo| !e.exiting);
    live.filter_map(|e| e.text.clone()).collect()
}

fn says(rt: &Runtime<Win>, words: &str) -> bool {
    texts(rt).iter().any(|t| t.contains(words))
}

fn fetches(sent: &Sent) -> Vec<String> {
    let asked = sent.borrow();
    let runs = asked.iter().filter_map(|effect| match effect {
        Effect::Source(_, SourceCmd::Run(run)) if run.command == GET => run.text.clone(),
        _ => None,
    });
    runs.collect()
}

/// A plugin folder of the test's own under the lane's `target/`.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/pl");
    let at = root.join(format!("{name}-{}", std::process::id()));
    if at.exists() {
        std::fs::remove_dir_all(&at)?;
    }
    std::fs::create_dir_all(at.join(NAME))?;
    Ok(at)
}

fn save(path: &Path, text: &str) -> Check {
    let before = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    std::fs::write(path, text)?;
    if let Some(before) = before {
        let file = std::fs::File::options().write(true).open(path)?;
        file.set_modified(before + Duration::from_secs(2))?;
    }
    Ok(())
}

#[test]
fn editing_the_script_redraws_bumps_the_version_and_keeps_the_store() -> Check {
    let panes = folder("edit")?;
    let dir = panes.join(NAME);
    save(&dir.join("plugin.json"), MANIFEST)?;
    save(&dir.join("pane.luau"), COUNTER)?;
    let (mut rt, _) = draw(|hub| open(hub, panes.clone()))?;
    at(&mut rt, 0.0);
    assert!(says(&rt, "Runs"), "first picture: {:?}", texts(&rt));
    assert!(says(&rt, "live · version 1"));
    save(
        &dir.join("pane.luau"),
        &COUNTER.replace("\"Runs\"", "\"Runs so far\""),
    )?;
    at(&mut rt, 1.5);
    assert!(says(&rt, "Runs so far"), "after the edit: {:?}", texts(&rt));
    assert!(says(&rt, "live · version 2"));
    assert!(says(&rt, "2"), "the store kept its count: {:?}", texts(&rt));
    let kept = std::fs::read_to_string(dir.join("store.json"))?;
    assert_eq!(kept, "{\"runs\":2}");
    Ok(())
}

#[test]
fn a_store_comes_back_after_a_restart() -> Check {
    let plugin = Plugin::new(NAME, MANIFEST, COUNTER, "{\"runs\":41}");
    assert_eq!(plugin.store().get("runs"), Some(&Data::Integer(41)));
    Ok(())
}

#[test]
fn a_script_that_loops_forever_is_stopped_and_says_so() -> Check {
    let (mut rt, _) = draw(|hub| {
        if let Some(library) = library(hub) {
            library.add(Plugin::new(NAME, MANIFEST, FOREVER, ""));
        }
    })?;
    at(&mut rt, 0.0);
    let said = texts(&rt).join(" ");
    assert!(said.contains("time limit"), "said: {said}");
    Ok(())
}

#[test]
fn a_host_not_granted_or_switched_off_is_never_fetched() -> Check {
    let (mut rt, sent) = draw(|hub| {
        if let Some(library) = library(hub) {
            library.add(Plugin::new(NAME, MANIFEST, ASKS, ""));
        }
    })?;
    at(&mut rt, 0.0);
    assert_eq!(
        fetches(&sent),
        Vec::<String>::new(),
        "nothing is switched on"
    );
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            super::library::grant(&mut hub.model, NAME, "ci-ubuntu:9100", true);
        }
    });
    at(&mut rt, 0.5);
    assert_eq!(
        fetches(&sent),
        vec![URL.to_owned()],
        "only the granted host"
    );
    Ok(())
}

#[test]
fn an_answer_arriving_redraws_with_it() -> Check {
    let (mut rt, sent) = draw(|hub| {
        super::library::grant(&mut hub.model, NAME, "ci-ubuntu:9100", true);
        if let Some(library) = library(hub) {
            library.add(Plugin::new(NAME, MANIFEST, ASKS, ""));
        }
    })?;
    at(&mut rt, 0.0);
    assert!(says(&rt, "-"));
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            answered(&mut hub);
        }
    });
    at(&mut rt, 0.2);
    assert!(
        texts(&rt).iter().any(|t| t == "3"),
        "waiting: {:?}",
        texts(&rt)
    );
    assert_eq!(
        fetches(&sent).len(),
        1,
        "asked once, not again on the answer"
    );
    Ok(())
}

#[test]
fn timer_runs_stop_while_the_pane_is_not_shown() -> Check {
    let (mut rt, sent) = draw(|hub| {
        super::library::grant(&mut hub.model, NAME, "ci-ubuntu:9100", true);
        if let Some(library) = library(hub) {
            library.add(Plugin::new(NAME, MANIFEST, ASKS, ""));
        }
    })?;
    at(&mut rt, 0.0);
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            answered(&mut hub);
        }
    });
    at(&mut rt, 0.1);
    at(&mut rt, 5.5);
    assert_eq!(fetches(&sent).len(), 2, "every 5 s asks again while shown");
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            hub.stage = Stage::App;
        }
    });
    at(&mut rt, 11.0);
    assert_eq!(fetches(&sent).len(), 2, "hidden, the timer is still");
    Ok(())
}

#[test]
fn a_manifest_with_a_field_this_app_does_not_know_loads() -> Check {
    let read = Manifest::read(MANIFEST)?;
    assert_eq!(read.name, NAME);
    assert_eq!(read.reach, vec!["ci-ubuntu:9100".to_owned()]);
    assert_eq!(read.every, Some(5));
    assert!(read.raw.get("adds").is_some(), "the unknown field is kept");
    assert!(Manifest::read("{\"name\": \"x\", \"reach\": [\"no-port\"]}").is_err());
    Ok(())
}

#[test]
fn a_bad_manifest_is_a_row_with_its_error_not_a_crash() -> Check {
    let plugin = Plugin::new(NAME, "{ not json", COUNTER, "");
    let row = plugin.row();
    assert!(
        row.words("description")
            .is_some_and(|d| d.starts_with("plugin.json does not read"))
    );
    Ok(())
}

/// A thread the window shows, made the way a client sees one made.
fn thread(hub: &mut Hub) -> Check {
    let lines = [
        r#"{"dir":"recv","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}}"#,
        r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}}"#,
        r#"{"dir":"recv","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"t-1"}}}"#,
    ];
    for line in lines {
        yd_core::wire::replay::play_line(&mut hub.model, line)?;
    }
    Ok(())
}

#[test]
fn build_sends_one_prompt_that_names_the_folder() -> Check {
    let panes = folder("build")?;
    let mut made = Ok(());
    let (mut rt, sent) = draw(|hub| {
        open(hub, panes.clone());
        made = thread(hub);
    })?;
    made?;
    let mut built = false;
    rt.update(|win, _| built = build(win, "Make me a pane of the upload queue"));
    assert!(built, "a thread and a folder are there");
    let prompts: Vec<String> = sent
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Effect::Wire(request) => Some(request.line.clone()),
            _ => None,
        })
        .filter(|line| line.contains("session/prompt"))
        .collect();
    assert_eq!(prompts.len(), 1, "one prompt: {prompts:?}");
    let named = panes.display().to_string();
    assert!(
        prompts
            .iter()
            .all(|p| p.contains(&named) && p.contains("upload queue"))
    );
    Ok(())
}

#[test]
fn the_queue_depth_scene_shows_frame_thirteens_pane() -> Check {
    let setup = SCENES.first().ok_or("no scene")?.setup;
    let (mut rt, _) = draw(setup)?;
    at(&mut rt, 0.0);
    let said = texts(&rt);
    let wanted = [
        "Queue depth",
        "ci-ubuntu",
        "Waiting",
        "Retrying",
        "41",
        "Done",
        "Files",
        "last 10 minutes above",
        "Name",
        "State",
        "Tries",
        "detail-front.png",
        "retrying",
        "detail-back.png",
        "waiting",
        "0",
        "swatch-olive.png",
        "swatch-sand.png",
        "from plugin queue-depth · every 5 s",
        "Yi built this",
        "live · version 2",
    ];
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|w| !said.iter().any(|t| t == w))
        .collect();
    assert!(missing.is_empty(), "missing {missing:?} from {said:?}");
    Ok(())
}

#[test]
fn a_script_that_fails_shows_its_line_over_the_last_picture() -> Check {
    let setup = SCENES.get(1).ok_or("no scene")?.setup;
    let (mut rt, _) = draw(setup)?;
    at(&mut rt, 0.0);
    let said = texts(&rt).join(" | ");
    assert!(said.contains("pane.luau:6"), "the line is named: {said}");
    assert!(
        said.contains("detail-front.png"),
        "the last picture stays: {said}"
    );
    Ok(())
}

const GREEDY: &str = r#"
local pane = require("@yi/pane")
return { view = function(ctx)
    local n = (ctx.store.get("runs") or 0) + 1
    ctx.store.set("runs", n)
    for i = 1, 20 do ctx.http.get("http://ci-ubuntu:9100/q" .. tostring(n * 100 + i)) end
    return { pane.stat("Runs", n) }
end }
"#;

fn greedy(hub: &mut Hub) {
    super::library::grant(&mut hub.model, NAME, "ci-ubuntu:9100", true);
    if let Some(library) = library(hub) {
        library.add(Plugin::new(NAME, MANIFEST, GREEDY, ""));
    }
}

/// Puts an answer for `url` in the fetch table, as the fetch source does.
fn reply(rt: &mut Runtime<Win>, url: &str) {
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let row = yd_core::row::Row::new(["get:", url].concat())
                .text("url", url)
                .text("body", "{}");
            yd_core::msg::step(
                &mut hub.model,
                yd_core::msg::Msg::Feed(
                    yd_core::feed::FETCH,
                    yd_core::feed::FeedEvent::Upsert(row),
                ),
            );
        }
    });
}

fn runs(rt: &Runtime<Win>) -> Result<Data, Box<dyn Error>> {
    let mut hub = rt.state.hub.try_borrow_mut()?;
    let plugin = library(&mut hub)
        .and_then(|l| l.find(NAME))
        .ok_or("no plugin")?;
    Ok(plugin.store().get("runs").cloned().unwrap_or_default())
}

#[test]
fn a_script_asking_twenty_addresses_fetches_eight_and_says_so() -> Check {
    let (mut rt, sent) = draw(greedy)?;
    at(&mut rt, 0.0);
    assert_eq!(fetches(&sent).len(), 8);
    assert!(says(&rt, "kept 8 of 20 web asks"), "{:?}", texts(&rt));
    Ok(())
}

#[test]
fn an_answer_reruns_only_a_shown_plugin_that_asked_for_it() -> Check {
    let (mut rt, sent) = draw(greedy)?;
    at(&mut rt, 0.0);
    reply(&mut rt, "http://elsewhere:80/");
    at(&mut rt, 0.1);
    assert_eq!(
        runs(&rt)?,
        Data::Integer(1),
        "another plugin's answer ran it"
    );
    let own = fetches(&sent).first().cloned().ok_or("nothing asked")?;
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            hub.stage = yd_ui::pane::Stage::App;
        }
    });
    reply(&mut rt, &own);
    at(&mut rt, 0.2);
    assert_eq!(
        runs(&rt)?,
        Data::Integer(1),
        "a hidden pane ran on its answer"
    );
    rt.update(|win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            hub.stage = Stage::Pane(PaneKind::Script);
        }
    });
    at(&mut rt, 0.3);
    assert_eq!(
        runs(&rt)?,
        Data::Integer(2),
        "shown again, its answer draws"
    );
    Ok(())
}

#[test]
fn a_script_asking_a_new_address_each_run_stops_at_the_address_cap() -> Check {
    let (mut rt, sent) = draw(greedy)?;
    at(&mut rt, 0.0);
    for n in 1..20 {
        let asked = fetches(&sent);
        for url in &asked {
            reply(&mut rt, url);
        }
        at(&mut rt, f64::from(n) * 0.1);
    }
    assert_eq!(
        fetches(&sent).len(),
        32,
        "one load reaches at most 32 addresses"
    );
    Ok(())
}

/// The fetch table is one for every plugin: past its cap the oldest answers go, but one a
/// plugin still draws from outlives newer ones nobody reads.
#[test]
fn the_fetch_table_keeps_sixty_four_answers_and_the_oldest_nobody_waits_on_goes_first() -> Check {
    use super::library::MAX_FETCHED;
    use yd_core::feed::FETCH;

    let (mut rt, _) = draw(|hub| {
        answered(hub);
        if let Some(library) = library(hub) {
            library.add(Plugin::new(NAME, MANIFEST, ASKS, ""));
        }
    })?;
    at(&mut rt, 0.0);
    let stranger = |n: usize| format!("http://elsewhere:80/{n}");
    for n in 0..MAX_FETCHED + 6 {
        reply(&mut rt, &stranger(n));
    }
    at(&mut rt, 0.1);
    let hub = rt.state.hub.try_borrow()?;
    let table = hub.model.tables.get(FETCH).ok_or("no fetch table")?;
    let held = |url: &str| {
        let id = yd_core::ids::RowId::new(["get:", url].concat());
        table.find(&id).is_some()
    };
    assert_eq!(yd_core::row::Rows::len(table), MAX_FETCHED);
    assert!(
        held(URL),
        "the oldest answer went while its plugin reads it"
    );
    assert!(
        !held(&stranger(0)) && !held(&stranger(6)),
        "the oldest stay"
    );
    assert!(held(&stranger(7)) && held(&stranger(MAX_FETCHED + 5)));
    Ok(())
}

#[test]
fn a_plugin_write_never_goes_through_a_link_at_its_temporary_name() -> Check {
    let dir = folder("planted")?;
    let (other, path) = (dir.join("other.txt"), dir.join(NAME).join("store.json"));
    std::fs::write(&other, "keep me")?;
    std::os::unix::fs::symlink(&other, path.with_extension("json.new"))?;
    library::write(&path, "{}")?;
    assert_eq!(std::fs::read_to_string(&other)?, "keep me");
    assert_eq!(std::fs::read_to_string(&path)?, "{}");
    assert!(!std::fs::symlink_metadata(&path)?.is_symlink());
    Ok(())
}
