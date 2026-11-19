//! The launchd source against recorded output: no test runs `launchctl`.

use std::cell::RefCell;
use std::error::Error;

use yd_core::address::uri::Address;
use yd_core::inbox::inbox;
use yd_core::model::Model;

use super::*;

type Check = Result<(), Box<dyn Error>>;

const LIST: &str = "PID\tStatus\tLabel\n-\t0\tcom.apple.SafariHistoryServiceAgent\n\
    99235\t-9\tcom.apple.progressd\n-\t0\tcom.apple.named\n4412\t0\tcom.yi.serve\n4530\t-9\tyi.kernel\n\
    -\t1\tcom.yi.figma-mcp\n-\t-9\tcom.yi.crashy\n-\t0\tcom.yi.noisy\n\
    -\t0\tcom.example.in-project\n-\t0\tcom.example.in-settings\n-\t0\tcom.example.other\n";
const DISABLED: &str = "disabled services = {\n\t\"com.yi.noisy\" => disabled\n}\n";
const FIXTURE: &str = include_str!("../../../../fixtures/mockup/daemons.rows.json");

/// A folder of property lists, already as `plutil -convert json` prints them, and the two
/// logs they name.
fn folder(name: &str) -> Result<Scope, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/launchd-tests");
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    let put = |stem: &str, label: &str, run: bool, err: Option<&str>| {
        let log = err.map_or(String::new(), |e| {
            format!(r#","StandardErrorPath":"DIR/{e}""#)
        });
        let json = format!(r#"{{"Label":"{label}","RunAtLoad":{run}{log}}}"#);
        let json = json.replace("DIR", &dir.display().to_string());
        std::fs::write(dir.join(format!("{stem}.plist")), json)
    };
    put("serve", "com.yi.serve", true, Some("serve.err"))?;
    put("figma", "com.yi.figma-mcp", true, Some("figma.err"))?;
    put("forge", "yi.forge-runner", false, None)?;
    put("noisy", "com.yi.noisy", true, None)?;
    let tool = r#"{"Label":"com.example.in-project","Program":"/work/proj/bin/t"}"#;
    std::fs::write(dir.join("tool.plist"), tool)?;
    std::fs::write(dir.join("broken.plist"), "{ not json")?;
    let lines = |n: usize, w: &str| (1..=n).map(|i| format!("{w} {i}\n")).collect::<String>();
    std::fs::write(dir.join("serve.err"), lines(250, "out"))?;
    std::fs::write(
        dir.join("figma.err"),
        lines(25, "line") + "token expired\u{7} 17:02\n",
    )?;
    let named = ["com.example.in-settings", "com.apple.named"].map(str::to_owned);
    Ok(Scope {
        agents: Some(dir),
        project: Some("/work/proj".into()),
        named: named.to_vec(),
    })
}

/// Answers with the recorded output of each command and keeps what it was asked.
fn recorded(
    calls: &RefCell<Vec<String>>,
) -> impl FnMut(&str, &[&str]) -> Result<Vec<u8>, RunError> + '_ {
    move |program, args| {
        let said = format!("{program} {}", args.join(" "));
        calls.borrow_mut().push(said);
        let text = match (program, args) {
            ("launchctl", ["list"]) => LIST.as_bytes().to_vec(),
            ("launchctl", ["print-disabled", _]) => DISABLED.as_bytes().to_vec(),
            ("id", ["-u"]) => b"501\n".to_vec(),
            ("plutil", ["-convert", "json", "-o", "-", path]) => {
                std::fs::read(path).map_err(|e| RunError(e.to_string()))?
            }
            _ => Vec::new(),
        };
        Ok(text)
    }
}

fn rows(name: &str) -> Result<Reading, Box<dyn Error>> {
    let (scope, calls) = (folder(name)?, RefCell::new(Vec::new()));
    Ok(read(
        &mut recorded(&calls),
        &scope,
        Uid(501),
        &mut Plists::default(),
    )?)
}

fn row<'a>(rows: &'a [Row], id: &str) -> Result<&'a Row, Box<dyn Error>> {
    rows.iter()
        .find(|r| r.id.as_str() == id)
        .ok_or_else(|| format!("no row {id}").into())
}

#[test]
fn a_running_service_has_the_fixture_s_field_names_and_values() -> Check {
    let rows = rows("fields")?.rows;
    let fixture: Vec<Json> = serde_json::from_str(FIXTURE)?;
    let cells = fixture
        .iter()
        .filter_map(|row| row.get("cells")?.as_object());
    let allowed: BTreeSet<&str> = cells
        .flat_map(|cells| cells.keys().map(String::as_str))
        .collect();
    for row in rows
        .iter()
        .filter(|r| !r.id.as_str().starts_with("notice:"))
    {
        assert!(
            row.cells.keys().all(|f| allowed.contains(f.as_str())),
            "{:?}",
            row.cells.keys()
        );
    }
    let serve = row(&rows, "com.yi.serve")?;
    assert_eq!(serve.get("state"), Some(&Value::Dot(Dot::Run)));
    assert_eq!(serve.words("pid"), Some("4412"));
    assert_eq!(serve.get("login"), Some(&Value::Flag(true)));
    let tags = Value::Tags(["Restart", "Stop", "Logs"].map(str::to_owned).to_vec());
    assert_eq!(serve.get("actions"), Some(&tags));
    assert_eq!(
        (serve.get("up"), serve.get("restarts")),
        (None, None),
        "launchd gives neither"
    );
    Ok(())
}

#[test]
fn a_failed_service_says_how_it_failed_and_shows_the_tail_of_its_error_log() -> Check {
    let rows = rows("tail")?.rows;
    let crashy = row(&rows, "com.yi.crashy")?;
    assert_eq!(crashy.words("last_line"), Some("killed by signal KILL (9)"));
    assert_eq!(
        row(&rows, "yi.kernel")?.get("state"),
        Some(&Value::Dot(Dot::Run)),
        "a pid beats an old status"
    );
    let figma = row(&rows, "com.yi.figma-mcp")?;
    assert_eq!(
        figma.words("last_line"),
        Some("exit 1, token expired 17:02")
    );
    let note: Vec<&str> = figma.words("note").ok_or("no note")?.lines().collect();
    let cut = "[… last 20 of 26 lines, cap 20; service.logs gives 200]";
    assert_eq!(
        (note.first(), note.get(1), note.len()),
        (Some(&cut), Some(&"line 7"), 21)
    );
    assert_eq!(note.last(), Some(&"token expired 17:02"));
    Ok(())
}

#[test]
fn only_the_users_own_services_are_listed_and_what_is_skipped_is_counted() -> Check {
    let rows = rows("scope")?.rows;
    let mut ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    let want = [
        "com.example.in-project",
        "com.example.in-settings",
        "com.yi.crashy",
        "com.yi.figma-mcp",
    ];
    let more = [
        "com.yi.noisy",
        "com.yi.serve",
        "notice:skipped",
        "yi.forge-runner",
        "yi.kernel",
    ];
    assert_eq!(
        ids,
        [want.as_slice(), more.as_slice()].concat(),
        "no com.apple., not even named"
    );
    let skipped = "[… 1 skipped: not readable as a service or property list]";
    assert_eq!(row(&rows, "notice:skipped")?.words("name"), Some(skipped));
    Ok(())
}

#[test]
fn a_service_that_is_not_loaded_shows_stopped_and_login_follows_launchctl() -> Check {
    let got = rows("stopped")?;
    let forge = row(&got.rows, "yi.forge-runner")?;
    assert_eq!(forge.get("state"), Some(&Value::Dot(Dot::Stop)));
    assert_eq!(
        forge.get("actions"),
        Some(&Value::Tags(vec!["Start".into()]))
    );
    assert_eq!(forge.words("at_login"), Some("does not start"));
    let noisy = row(&got.rows, "com.yi.noisy")?;
    assert_eq!(
        noisy.get("login"),
        Some(&Value::Flag(false)),
        "disabled beats RunAtLoad"
    );
    assert!(
        got.known
            .get("yi.forge-runner")
            .is_some_and(|k| !k.loaded && k.file.is_some())
    );
    Ok(())
}

#[test]
fn each_action_names_exactly_these_arguments() {
    let loaded = Known {
        loaded: true,
        login: true,
        ..Known::default()
    };
    let idle = Known {
        file: Some("/agents/a.plist".into()),
        ..Known::default()
    };
    let cases: [(CommandId, &Known, &[&str]); 8] = [
        (service::RESTART, &loaded, &["kickstart", "-k", "gui/501/a"]),
        (service::STOP, &loaded, &["bootout", "gui/501/a"]),
        (service::START, &loaded, &["kickstart", "gui/501/a"]),
        (
            service::START,
            &idle,
            &["bootstrap", "gui/501", "/agents/a.plist"],
        ),
        (service::LOGIN, &loaded, &["disable", "gui/501/a"]),
        (service::LOGIN, &idle, &["enable", "gui/501/a"]),
        (service::LOGS, &loaded, &[]),
        (service::START, &Known::default(), &[]),
    ];
    for (command, known, want) in cases {
        let got = action_args(&command, Uid(501), "a", known);
        let want = (!want.is_empty()).then(|| want.iter().map(|w| (*w).to_owned()).collect());
        assert_eq!(got, want, "{command:?}");
    }
}

fn serve_with(
    name: &str,
    sent: &[(&str, CommandId)],
) -> Result<(Vec<String>, Model), Box<dyn Error>> {
    let (scope, calls) = (folder(name)?, RefCell::new(Vec::new()));
    let ((tx, inbox), (cmds_tx, cmds)) = (inbox(|| {}), std::sync::mpsc::channel());
    for (label, command) in sent {
        let target = Target::Thing(Address::new(Kind::Service, *label)?);
        cmds_tx.send(SourceCmd::Run(Run::new(*command).on(target)))?;
    }
    drop(cmds_tx);
    serve(
        tx.feed(FEED),
        cmds,
        &mut recorded(&calls),
        scope,
        Duration::from_millis(5),
    )?;
    let mut model = Model::default();
    inbox.drain(&mut model);
    Ok((calls.take(), model))
}

#[test]
fn a_stop_runs_one_bootout_and_a_service_it_never_listed_runs_nothing() -> Check {
    let sent = [
        ("com.yi.serve", service::STOP),
        ("com.apple.progressd", service::STOP),
        ("nope", service::RESTART),
    ];
    let (calls, _) = serve_with("serve-stop", &sent)?;
    let acted: Vec<&String> = calls
        .iter()
        .filter(|c| c.contains("bootout") || c.contains("kickstart"))
        .collect();
    assert_eq!(acted, ["launchctl bootout gui/501/com.yi.serve"]);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "id -u").count(),
        1,
        "the user id is read once"
    );
    Ok(())
}

#[test]
fn service_logs_sends_the_last_200_lines_and_names_the_cut() -> Check {
    let (_, model) = serve_with("serve-logs", &[("com.yi.serve", service::LOGS)])?;
    let table = model.tables.get(FEED).ok_or("no table")?;
    let said = |n: usize| {
        table
            .find(&RowId::new(format!("log:com.yi.serve:{n}")))
            .and_then(|r| r.words("name"))
    };
    let line = table.find(&RowId::new("log:com.yi.serve:1"));
    assert!(line.is_some_and(|r| r.dim && r.parent == Some(RowId::new("com.yi.serve"))));
    let cut = "[… last 200 of 250 lines, cap 200; the log file holds the rest]";
    assert_eq!(
        (said(0), said(1), said(200), said(201)),
        (Some(cut), Some("out 51"), Some("out 250"), None)
    );
    Ok(())
}

/// A label is one argument inside `gui/<uid>/<label>`. One with a space or a slash is never a
/// row, so nothing can aim a command at it, and a command aimed at it anyway runs nothing.
#[test]
fn a_label_that_is_not_plain_text_is_not_listed_and_never_reaches_an_argument() -> Check {
    let list = "PID\tStatus\tLabel\n7\t0\tcom.yi.ok\n8\t0\tcom.yi.a b\n9\t0\tcom.yi/../x\n";
    let calls = RefCell::new(Vec::new());
    let mut runner = |program: &str, args: &[&str]| {
        calls
            .borrow_mut()
            .push(format!("{program} {}", args.join(" ")));
        Ok(match (program, args) {
            ("launchctl", ["list"]) => list.as_bytes().to_vec(),
            ("id", ["-u"]) => b"501\n".to_vec(),
            _ => Vec::new(),
        })
    };
    let ((tx, inbox), (cmds_tx, cmds)) = (inbox(|| {}), std::sync::mpsc::channel());
    cmds_tx.send(SourceCmd::Refresh)?;
    for label in ["com.yi.a b", "com.yi/../x"] {
        let target = Target::Thing(Address::new(Kind::Service, label)?);
        cmds_tx.send(SourceCmd::Run(Run::new(service::STOP).on(target)))?;
    }
    drop(cmds_tx);
    let scope = Scope::default();
    serve(
        tx.feed(FEED),
        cmds,
        &mut runner,
        scope,
        Duration::from_secs(60),
    )?;
    let mut model = Model::default();
    inbox.drain(&mut model);
    let table = model.tables.get(FEED).ok_or("no table")?;
    let listed = |id: &str| table.find(&RowId::new(id)).is_some();
    assert!(listed("com.yi.ok") && !listed("com.yi.a b") && !listed("com.yi/../x"));
    let skipped = table.find(&RowId::new("notice:skipped"));
    let said = skipped
        .and_then(|row| row.words("name"))
        .unwrap_or_default();
    assert!(said.starts_with("[… 2 skipped"), "{said}");
    let acted = calls.borrow().iter().any(|call| call.contains("bootout"));
    assert!(!acted, "{:?}", calls.borrow());
    Ok(())
}

#[test]
fn a_property_list_is_read_again_only_when_its_file_changed() -> Check {
    let (scope, calls) = (folder("plist-cache")?, RefCell::new(Vec::new()));
    let mut seen = Plists::default();
    let plutils = |seen: &mut Plists| -> Result<Vec<String>, Box<dyn Error>> {
        calls.borrow_mut().clear();
        read(&mut recorded(&calls), &scope, Uid(501), seen)?;
        let ran = calls.borrow().clone();
        Ok(ran
            .into_iter()
            .filter(|call| call.starts_with("plutil"))
            .collect())
    };
    assert_eq!(
        plutils(&mut seen)?.len(),
        6,
        "every list is read the first time"
    );
    assert_eq!(plutils(&mut seen)?, Vec::<String>::new(), "none changed");
    let dir = scope.agents.clone().ok_or("no folder")?;
    let forge = dir.join("forge.plist");
    std::fs::write(&forge, r#"{"Label":"yi.forge-runner","RunAtLoad":true}"#)?;
    let again = plutils(&mut seen)?;
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(again[0].ends_with("forge.plist"), "{again:?}");
    Ok(())
}

/// With a 20 ms timer: nothing is read before a pane shows the feed, the labels of the
/// setting replace those the source had, and the timer stops when the last pane closes.
#[test]
fn the_source_reads_on_its_timer_only_while_a_pane_shows_its_feed() -> Check {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let looks = AtomicUsize::new(0);
    let mut runner = |program: &str, args: &[&str]| {
        Ok(match (program, args) {
            ("launchctl", ["list"]) => {
                looks.fetch_add(1, SeqCst);
                LIST.as_bytes().to_vec()
            }
            ("id", ["-u"]) => b"501\n".to_vec(),
            _ => Vec::new(),
        })
    };
    let ((tx, inbox), (cmds, rx)) = (inbox(|| {}), std::sync::mpsc::channel());
    let feed = tx.feed(FEED);
    let rest = |ms| std::thread::sleep(Duration::from_millis(ms));
    let until = |n: usize| {
        (0..500).any(|_| {
            rest(5);
            looks.load(SeqCst) >= n
        })
    };
    std::thread::scope(|scope| -> Check {
        let every = Duration::from_millis(20);
        let source = scope.spawn(move || serve(feed, rx, &mut runner, Scope::default(), every));
        cmds.send(SourceCmd::Labels(vec!["com.example.other".to_owned()]))?;
        rest(120);
        assert_eq!(looks.load(SeqCst), 0, "read with no pane showing the feed");
        cmds.send(SourceCmd::Seen(true))?;
        assert!(
            until(3),
            "the timer did not run while a pane showed the feed"
        );
        cmds.send(SourceCmd::Seen(false))?;
        rest(80);
        let hidden = looks.load(SeqCst);
        rest(160);
        assert_eq!(
            looks.load(SeqCst),
            hidden,
            "read on after the last pane closed"
        );
        cmds.send(SourceCmd::Refresh)?;
        assert!(until(hidden + 1), "Refresh did not read at once");
        drop(cmds);
        source.join().map_err(|_| "the source died")??;
        Ok(())
    })?;
    let mut model = Model::default();
    inbox.drain(&mut model);
    let table = model.tables.get(FEED).ok_or("no table")?;
    let listed = |id: &str| table.find(&RowId::new(id)).is_some();
    assert!(listed("com.example.other"), "the setting's label is listed");
    assert!(!listed("com.example.in-settings"), "a label nobody named");
    Ok(())
}
