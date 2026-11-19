use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::feed::{SourceCmd, Table};
use yd_core::ids::{CommandId, RowId};
use yd_core::inbox::{Inbox, inbox};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::row::{Dot, Row, Rows, Value};

use super::{
    APPLY_FIX, DEFINITION, FEED, FIXES, HINTS, HOVER, LANGUAGES, Language, OPEN, REFERENCES,
    RESTART, serve,
};
use crate::fs::tests::scratch;

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/lsp/fake/server.py");
const LIB: &str = "fn main() {\n    let café = \"🦀\"; let x = 1;\n    let y = x + 1;\n}\n";

const fn fake(args: &'static [&'static str]) -> [Language; 1] {
    [Language {
        server: "fake",
        language: "Rust",
        id: "rust",
        extensions: &["rs"],
        program: "python3",
        args,
        options: serde_json::Value::default,
    }]
}

macro_rules! script {
    ($name:literal) => {
        &[
            FAKE,
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/lsp/fake/", $name),
        ]
    };
}

const BASIC: &[Language] = &fake(script!("basic.json"));
const SUPERSEDE: &[Language] = &fake(script!("supersede.json"));
const HUGE: &[Language] = &fake(script!("huge.json"));
const DIES: &[Language] = &fake(script!("dies.json"));
const STUBBORN: &[Language] = &fake(script!("stubborn.json"));
const QUIET: &[Language] = &fake(script!("quiet.json"));
const MISSING: &[Language] = &[Language {
    server: "no-such-server",
    language: "Rust",
    id: "rust",
    extensions: &["rs"],
    program: "yd-no-such-language-server",
    args: &[],
    options: serde_json::Value::default,
}];
const GUARDED: &[Language] = &[Language {
    options: LANGUAGES[0].options,
    ..fake(script!("basic.json"))[0]
}];

struct Lsp {
    cmds: Option<Sender<SourceCmd>>,
    inbox: Inbox,
    model: Model,
    root: PathBuf,
    thread: Option<JoinHandle<()>>,
}

fn start(name: &str, languages: &'static [Language]) -> Result<Lsp, Box<dyn Error>> {
    let root = scratch(&format!("lsp-{name}"))?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/lib.rs"), LIB)?;
    let (sender, inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    let out = sender.feed(FEED);
    let thread = std::thread::spawn(move || {
        let _ = serve(out, rx, languages);
    });
    cmds.send(SourceCmd::Root(root.clone()))?;
    Ok(Lsp {
        cmds: Some(cmds),
        inbox,
        model: Model::default(),
        root,
        thread: Some(thread),
    })
}

impl Lsp {
    fn run(&self, command: CommandId, part: &str) -> Result<(), Box<dyn Error>> {
        let address = Address::new(Kind::Lines, "src/lib.rs")?.part(part);
        let run = Run::new(command).on(Target::Thing(address));
        let cmds = self.cmds.as_ref().ok_or("closed")?;
        Ok(cmds.send(SourceCmd::Run(run))?)
    }

    fn table(&self) -> Table {
        self.model.tables.get(FEED).cloned().unwrap_or_default()
    }

    fn until(
        &mut self,
        what: &str,
        done: impl Fn(&Table) -> bool,
    ) -> Result<Table, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.inbox.drain(&mut self.model);
            let table = self.table();
            if done(&table) {
                return Ok(table);
            }
            if Instant::now() > deadline {
                return Err(format!("timed out waiting for {what}: {table:#?}").into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn row(&mut self, id: &str) -> Result<Row, Box<dyn Error>> {
        let id = RowId::new(id);
        let table = self.until(id.as_str(), |t| t.find(&id).is_some())?;
        Ok(table.find(&id).cloned().ok_or("gone")?)
    }

    fn pid(&self) -> Result<i32, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Incident: the server makes the file, then writes it; read between, it is empty.
            let written = std::fs::read_to_string(self.root.join(".fake.pid"));
            if let Some(pid) = written.ok().and_then(|pid| pid.trim().parse().ok()) {
                return Ok(pid);
            }
            if Instant::now() > deadline {
                return Err("the fake server never started in the root".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn text<'a>(row: &'a Row, field: &str) -> &'a str {
    row.words(field).unwrap_or("")
}

fn gone(pid: i32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        let alive = rustix::process::Pid::from_raw(pid)
            .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok());
        if !alive {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn rust_analyzer_is_started_with_build_scripts_proc_macros_and_check_off()
-> Result<(), Box<dyn Error>> {
    let mut lsp = start("guarded", GUARDED)?;
    lsp.run(OPEN, "L1")?;
    let id = RowId::new("server:fake");
    let ready = |r: &Row| r.get("state") == Some(&Value::Dot(Dot::Run));
    lsp.until("the server to be ready", |t| t.find(&id).is_some_and(ready))?;
    let sent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(lsp.root.join(".fake.init"))?)?;
    let off = serde_json::Value::Bool(false);
    assert_eq!(sent.pointer("/cargo/buildScripts/enable"), Some(&off));
    assert_eq!(sent.pointer("/procMacro/enable"), Some(&off));
    assert_eq!(sent.pointer("/checkOnSave"), Some(&off));
    Ok(())
}

#[test]
fn opening_a_file_starts_its_server_and_its_problems_arrive_in_character_columns()
-> Result<(), Box<dyn Error>> {
    let mut lsp = start("problems", BASIC)?;
    lsp.run(OPEN, "L1")?;
    let problem = lsp.row("problem:src/lib.rs:2:0")?;
    assert_eq!(text(&problem, "message"), "unused variable: `x`");
    assert_eq!(
        problem.get("severity"),
        Some(&Value::Badge("Warn".into(), yd_core::row::Tone::Warn))
    );
    assert_eq!(text(&problem, "where"), "src/lib.rs:2");
    assert_eq!((text(&problem, "line"), text(&problem, "col")), ("2", "25"));
    assert_eq!(text(&problem, "end_col"), "26");
    assert_eq!(text(&problem, "from"), "fake");
    let server = lsp.row("server:fake")?;
    assert_eq!(server.get("state"), Some(&Value::Dot(Dot::Run)));
    let Some(Value::Tags(can)) = server.get("can_do") else {
        return Err("no capabilities".into());
    };
    assert_eq!(
        can,
        &[
            "Problems",
            "Hover",
            "Go to definition",
            "Find references",
            "Rename",
            "Type hints",
            "Quick fixes"
        ]
    );
    assert_eq!(text(&server, "project"), "lsp-problems");
    Ok(())
}

#[test]
fn a_hover_asks_at_the_utf16_offset_of_its_character_column() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("hover", BASIC)?;
    lsp.run(HOVER, "L2:C25")?;
    let hover = lsp.row("hover:src/lib.rs:2:25")?;
    assert_eq!(text(&hover, "text"), "line 1 character 25");
    let answered = |t: &Table| {
        t.find(&RowId::new("server:fake"))
            .is_some_and(|r| text(r, "status").contains("answered 1 request,"))
    };
    lsp.until("the server to count the answer", answered)?;
    Ok(())
}

#[test]
fn references_answer_with_their_lines_and_replace_an_older_question() -> Result<(), Box<dyn Error>>
{
    let mut lsp = start("references", BASIC)?;
    lsp.run(REFERENCES, "L3:C13")?;
    let first = lsp.row("ref:src/lib.rs:3:13:1")?;
    assert_eq!(text(&first, "text"), "    let y = x + 1;");
    assert_eq!(
        (
            text(&first, "line"),
            text(&first, "col"),
            text(&first, "end_col")
        ),
        ("3", "13", "14")
    );
    lsp.run(REFERENCES, "L2:C25")?;
    let older = |t: &Table| t.find(&RowId::new("ref:src/lib.rs:3:13:0")).is_none();
    let table = lsp.until("the older references to go", |t| {
        older(t) && t.find(&RowId::new("ref:src/lib.rs:2:25:0")).is_some()
    })?;
    let at = table
        .find(&RowId::new("ref:src/lib.rs:2:25:0"))
        .ok_or("no ref")?;
    assert_eq!(
        (text(at, "line"), text(at, "col"), text(at, "kind")),
        ("2", "25", "read")
    );
    Ok(())
}

#[test]
fn a_definition_answers_with_where_it_is() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("definition", BASIC)?;
    lsp.run(DEFINITION, "L3:C13")?;
    let def = lsp.row("def:src/lib.rs:3:13")?;
    assert_eq!(
        (text(&def, "path"), text(&def, "line"), text(&def, "col")),
        ("src/lib.rs", "2", "25")
    );
    Ok(())
}

#[test]
fn hints_and_fixes_answer_and_a_fix_is_written_to_disk() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("fixes", BASIC)?;
    lsp.run(HINTS, "L1")?;
    let hint = lsp.row("hint:src/lib.rs:2:26")?;
    assert_eq!(text(&hint, "text"), ": i32");
    lsp.run(FIXES, "L2:C25")?;
    let fix = lsp.row("fix:src/lib.rs:2:0")?;
    assert_eq!(text(&fix, "title"), "Rename x to _x");
    let diff = text(&fix, "edit");
    assert!(
        diff.contains("-    let café = \"🦀\"; let x = 1;\n+    let café = \"🦀\"; let _x = 1;"),
        "{diff}"
    );
    let address = Address::new(Kind::Lines, "fix:src/lib.rs:2:0")?;
    let run = Run::new(APPLY_FIX).on(Target::Thing(address));
    lsp.cmds
        .as_ref()
        .ok_or("closed")?
        .send(SourceCmd::Run(run))?;
    let file = lsp.root.join("src/lib.rs");
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(&file)? == LIB {
        if Instant::now() > deadline {
            return Err("the fix never reached the file".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(&file)?,
        LIB.replace("let x", "let _x")
    );
    Ok(())
}

#[test]
fn a_newer_hover_for_the_same_file_cancels_the_older_one() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("supersede", SUPERSEDE)?;
    lsp.run(OPEN, "L1")?;
    let running = Value::Dot(Dot::Run);
    lsp.until("the server to start", |t| {
        t.find(&RowId::new("server:fake"))
            .and_then(|r| r.get("state"))
            == Some(&running)
    })?;
    lsp.run(HOVER, "L1:C1")?;
    lsp.run(HOVER, "L3:C5")?;
    let hover = lsp.row("hover:src/lib.rs:3:5")?;
    assert_eq!(text(&hover, "text"), "NEW");
    let counted = |t: &Table| {
        t.find(&RowId::new("server:fake"))
            .is_some_and(|r| text(r, "status").contains("answered"))
    };
    let table = lsp.until("the server row", counted)?;
    let server = table
        .find(&RowId::new("server:fake"))
        .ok_or("no server row")?;
    assert!(
        text(server, "status").contains("answered 1 request,"),
        "the old answer counted: {server:?}"
    );
    assert!(table.find(&RowId::new("hover:src/lib.rs:1:1")).is_none());
    Ok(())
}

#[test]
fn a_message_past_the_cap_ends_the_server_with_an_error() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("huge", HUGE)?;
    lsp.run(OPEN, "L1")?;
    let pid = lsp.pid()?;
    let failed = Value::Dot(Dot::Fail);
    let table = lsp.until("the server to fail", |t| {
        t.find(&RowId::new("server:fake"))
            .and_then(|r| r.get("state"))
            == Some(&failed)
    })?;
    let row = table.find(&RowId::new("server:fake")).ok_or("no row")?;
    assert!(
        text(row, "status").contains("past the 33554432-byte cap"),
        "{row:?}"
    );
    assert!(
        gone(pid, Duration::from_secs(5)),
        "the server is still running"
    );
    Ok(())
}

/// What rustup's `rust-analyzer` proxy does in a folder whose toolchain lacks the component,
/// as it printed on the build Mac.
const PROXY: &[Language] = &[Language {
    server: "proxy",
    program: "sh",
    args: &[
        "-c",
        "echo \"error: Unknown binary 'rust-analyzer' in official toolchain 'stable-aarch64-apple-darwin'.\" >&2; exit 1",
    ],
    ..fake(&[])[0]
}];

#[test]
fn a_server_that_exits_at_start_says_its_last_error_line() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("proxy", PROXY)?;
    lsp.run(OPEN, "L1")?;
    let failed = Value::Dot(Dot::Fail);
    let table = lsp.until("the server to fail", |t| {
        t.find(&RowId::new("server:proxy"))
            .and_then(|r| r.get("state"))
            == Some(&failed)
    })?;
    let row = table.find(&RowId::new("server:proxy")).ok_or("no row")?;
    let said = "Unknown binary 'rust-analyzer' in official toolchain";
    assert!(text(row, "status").contains(said), "{row:?}");
    Ok(())
}

#[test]
fn a_server_that_exits_mid_request_sets_an_error_status() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("dies", DIES)?;
    lsp.run(HOVER, "L2:C25")?;
    let failed = Value::Dot(Dot::Fail);
    let table = lsp.until("the server to fail", |t| {
        t.find(&RowId::new("server:fake"))
            .and_then(|r| r.get("state"))
            == Some(&failed)
    })?;
    let row = table.find(&RowId::new("server:fake")).ok_or("no row")?;
    assert!(
        text(row, "status").contains("exited with code 3"),
        "{row:?}"
    );
    assert!(text(row, "status").contains("1 request"), "{row:?}");
    Ok(())
}

#[test]
fn a_server_that_is_not_installed_is_a_row_saying_so() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("missing", MISSING)?;
    lsp.run(OPEN, "L1")?;
    let row = lsp.row("server:no-such-server")?;
    assert_eq!(row.get("state"), Some(&Value::Dot(Dot::Stop)));
    assert!(row.dim);
    assert!(text(&row, "status").starts_with("not installed"), "{row:?}");
    assert!(matches!(
        lsp.table().status(),
        yd_core::feed::FeedStatus::Live
    ));
    Ok(())
}

#[test]
fn a_new_root_stops_the_old_roots_server_even_one_that_ignores_exit() -> Result<(), Box<dyn Error>>
{
    let mut lsp = start("stubborn", STUBBORN)?;
    lsp.run(OPEN, "L1")?;
    lsp.row("server:fake")?;
    let pid = lsp.pid()?;
    let elsewhere = scratch("lsp-stubborn-elsewhere")?;
    lsp.cmds
        .as_ref()
        .ok_or("closed")?
        .send(SourceCmd::Root(elsewhere))?;
    assert!(
        gone(pid, Duration::from_secs(8)),
        "the old root's server is still running"
    );
    lsp.until("the old rows to go", |t| {
        t.find(&RowId::new("server:fake")).is_none()
    })?;
    Ok(())
}

#[test]
fn restart_starts_a_fresh_server_and_closing_stops_it() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("restart", BASIC)?;
    lsp.run(OPEN, "L1")?;
    lsp.row("server:fake")?;
    let first = lsp.pid()?;
    std::fs::remove_file(lsp.root.join(".fake.pid"))?;
    let address = Address::new(Kind::Lines, "server:fake")?;
    let run = Run::new(RESTART).on(Target::Thing(address));
    lsp.cmds
        .as_ref()
        .ok_or("closed")?
        .send(SourceCmd::Run(run))?;
    let second = lsp.pid()?;
    assert_ne!(first, second);
    assert!(
        gone(first, Duration::from_secs(5)),
        "the first server is still running"
    );
    lsp.row("problem:src/lib.rs:2:0")?;
    drop(lsp.cmds.take());
    let thread = lsp.thread.take().ok_or("no thread")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        thread.is_finished(),
        "the source did not end when its commands closed"
    );
    assert!(
        gone(second, Duration::from_secs(1)),
        "the server outlived its source"
    );
    Ok(())
}

fn status(lsp: &mut Lsp, said: &str) -> Result<(), Box<dyn Error>> {
    let wanted = yd_core::feed::FeedStatus::Error(said.to_owned());
    lsp.until(said, |t| *t.status() == wanted).map(|_| ())
}

#[test]
fn a_question_about_a_file_no_server_serves_is_answered_with_a_sentence()
-> Result<(), Box<dyn Error>> {
    let mut lsp = start("unserved", BASIC)?;
    std::fs::write(lsp.root.join("README.md"), "# Read me\n")?;
    let ask = |command, path: &str| -> Result<SourceCmd, Box<dyn Error>> {
        let at = Address::new(Kind::Lines, path)?.part("L1:C3");
        Ok(SourceCmd::Run(Run::new(command).on(Target::Thing(at))))
    };
    let cmds = lsp.cmds.clone().ok_or("closed")?;
    cmds.send(ask(REFERENCES, "README.md")?)?;
    status(&mut lsp, "no language server for .md files")?;
    cmds.send(ask(HOVER, "Makefile")?)?;
    let card = lsp.row("hover:Makefile:1:3")?;
    assert_eq!(text(&card, "text"), "no language server for Makefile");
    // Nobody waits on a file being opened or on its hints: each event moves the version, so
    // the table must stand where the one hover card puts it.
    let mut expected = lsp.table();
    for command in [OPEN, HINTS] {
        cmds.send(ask(command, "README.md")?)?;
    }
    cmds.send(ask(HOVER, "notes.txt")?)?;
    let said = Row::new("hover:notes.txt:1:3").text("text", "no language server for .txt files");
    lsp.row("hover:notes.txt:1:3")?;
    expected.apply(yd_core::feed::FeedEvent::Upsert(said));
    assert_eq!(
        lsp.table().version(),
        expected.version(),
        "opening a file nobody serves said something"
    );
    Ok(())
}

/// The scripted server holds its first hover for good: the question runs out of time.
#[test]
fn a_hover_that_times_out_says_so_in_its_card_not_the_feeds_status() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("late", SUPERSEDE)?;
    lsp.run(HOVER, "L1:C1")?;
    let card = lsp.row("hover:src/lib.rs:1:1")?;
    assert_eq!(text(&card, "text"), "no answer within 10 s");
    let table = lsp.table();
    assert!(
        !matches!(table.status(), yd_core::feed::FeedStatus::Error(_)),
        "{:?}",
        table.status()
    );
    let server = lsp.row("server:fake")?;
    assert!(
        text(&server, "status").contains("1 timed out"),
        "{server:?}"
    );
    Ok(())
}

/// A real rust-analyzer on a small crate: start, its own diagnostic, a hover. The crate
/// pins a toolchain that ships it; run the test binary without `RUSTUP_TOOLCHAIN`.
#[test]
#[ignore = "needs rust-analyzer on this Mac"]
fn rust_analyzer_reports_a_missing_module_and_answers_a_hover() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("real", super::LANGUAGES)?;
    let root: &Path = &lsp.root;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[workspace]\n",
    )?;
    std::fs::write(
        root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.97.1\"\n",
    )?;
    std::fs::write(
        root.join("src/lib.rs"),
        "mod missing;\npub fn f() -> u32 {\n    let n: u32 = 1;\n    n\n}\n",
    )?;
    lsp.run(OPEN, "L1")?;
    lsp.until("a problem", |t| !t.prefixed("problem:").is_empty())?;
    lsp.run(HOVER, "L3:C9")?;
    let hover = lsp.row("hover:src/lib.rs:3:9")?;
    assert!(text(&hover, "text").contains("u32"), "{hover:?}");
    Ok(())
}

/// The dogfood edit: a type error appended on disk to a file already open is reported once
/// the file is opened again, with no cargo check.
#[test]
#[ignore = "needs rust-analyzer on this Mac"]
fn rust_analyzer_reports_a_type_error_written_to_an_open_file() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("real-edit", super::LANGUAGES)?;
    let root: &Path = &lsp.root;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )?;
    let toolchain = "[toolchain]\nchannel = \"1.97.1\"\n";
    std::fs::write(root.join("rust-toolchain.toml"), toolchain)?;
    let good = "pub fn f() -> u32 {\n    1\n}\n";
    std::fs::write(root.join("src/lib.rs"), good)?;
    lsp.run(OPEN, "L1")?;
    let id = RowId::new("server:rust-analyzer");
    let ready = |r: &Row| r.get("state") == Some(&Value::Dot(Dot::Run));
    lsp.until("the server to be ready", |t| t.find(&id).is_some_and(ready))?;
    std::fs::write(
        lsp.root.join("src/lib.rs"),
        format!("{good}pub fn broken() -> u32 {{ \"x\" }}\n"),
    )?;
    lsp.run(OPEN, "L1")?;
    let said = |t: &Table| {
        let problems = t.prefixed("problem:src/lib.rs:4:");
        (0..problems.len()).any(|at| {
            problems
                .get(at)
                .is_some_and(|r| !text(r, "message").is_empty())
        })
    };
    lsp.until("the type error on line 4", said)?;
    Ok(())
}

/// The dogfood project, opened with its error already in it: no toolchain pinned.
#[test]
#[ignore = "needs rust-analyzer on this Mac"]
fn rust_analyzer_reports_an_error_a_file_had_when_it_opened() -> Result<(), Box<dyn Error>> {
    let mut lsp = start("real-early", super::LANGUAGES)?;
    let toml =
        "[package]\nname = \"tally\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n";
    std::fs::write(lsp.root.join("Cargo.toml"), toml)?;
    let lib = "use std::collections::BTreeMap;\n\npub fn f() -> BTreeMap<String, u32> {\n    BTreeMap::new()\n}\npub fn broken() -> u32 { \"x\" }\n";
    std::fs::write(lsp.root.join("src/lib.rs"), lib)?;
    lsp.run(OPEN, "L1")?;
    lsp.run(HINTS, "L1")?;
    lsp.until("the type error", |t| {
        !t.prefixed("problem:src/lib.rs:6:").is_empty()
    })?;
    Ok(())
}

#[test]
fn a_rustup_proxy_gives_way_to_a_toolchain_that_has_the_server() -> Result<(), Box<dyn Error>> {
    let home = scratch("lsp-rustup")?;
    let bin = home.join("cargo/bin");
    let has = home.join("rustup/toolchains/1.97.1-aarch64-apple-darwin/bin");
    let lacks = home.join("rustup/toolchains/stable-aarch64-apple-darwin/bin");
    for dir in [&bin, &has, &lacks] {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(bin.join("rustup"), "")?;
    std::os::unix::fs::symlink("rustup", bin.join("rust-analyzer"))?;
    std::fs::write(has.join("rust-analyzer"), "")?;
    let toolchains = home.join("rustup/toolchains");
    let found = super::server::unproxied(bin.join("rust-analyzer"), &toolchains);
    assert_eq!(found, has.join("rust-analyzer"));
    let plain = home.join("plain");
    std::fs::write(&plain, "")?;
    assert_eq!(super::server::unproxied(plain.clone(), &toolchains), plain);
    Ok(())
}

/// Incident: rust-analyzer told of a file before its workspace loaded reports none of the
/// file's errors once loaded; only the file told again after it says it is idle does.
#[test]
fn a_file_open_before_its_server_loaded_is_told_again_once_the_server_is_idle()
-> Result<(), Box<dyn Error>> {
    let mut lsp = start("quiet", QUIET)?;
    lsp.run(OPEN, "L1")?;
    let problem = lsp.row("problem:src/lib.rs:2:0")?;
    assert_eq!(text(&problem, "message"), "unused variable: `x`");
    Ok(())
}
