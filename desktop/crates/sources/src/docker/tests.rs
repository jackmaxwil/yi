//! The Docker source against recorded output: no test runs `docker`.

use std::cell::RefCell;
use std::error::Error;
use std::sync::mpsc::channel;

use yd_core::feed::Table;
use yd_core::ids::RowId;
use yd_core::inbox::inbox;
use yd_core::model::Model;
use yd_core::row::Rows;

use super::*;

type Check = Result<(), Box<dyn Error>>;
type Calls = RefCell<Vec<Vec<String>>>;

const PS: &str = concat!(
    r#"{"ID":"a1b2c3d4e5f6","Names":"atlas-api","Image":"atlas/api:rate-limit","State":"running","Status":"Up 14 minutes (healthy)","Ports":"0.0.0.0:8080->8080/tcp, [::]:8080->8080/tcp","Labels":"com.docker.compose.project=atlas,x=y"}"#, "\n",
    r#"{"ID":"b2c3d4e5f6a1","Names":"atlas-db","Image":"postgres:16","State":"running","Status":"Up 3 days","Ports":"5432/tcp","Labels":"com.docker.compose.project=atlas"}"#, "\n",
    r#"{"ID":"c3d4e5f6a1b2","Names":"registry","Image":"registry:2","State":"exited","Status":"Exited (137) 2 hours ago","Ports":"","Labels":""}"#, "\n",
);
const STATS: &str = concat!(
    r#"{"ID":"a1b2c3d4e5f6","Name":"atlas-api","CPUPerc":"33.52%","MemUsage":"212MiB / 7.67GiB"}"#, "\n",
    r#"{"ID":"b2c3d4e5f6a1","Name":"atlas-db","CPUPerc":"0.12%","MemUsage":"1.9GiB / 7.67GiB"}"#, "\n",
);

fn words<'a>(row: &'a Row, field: &str) -> &'a str { row.words(field).unwrap_or("") }

fn ok(stdout: &str) -> Result<Ran, RunError> { Ok(Ran { ok: true, stdout: stdout.into(), stderr: Vec::new() }) }

/// Recorded `ps` and `stats` output; every other call is noted in `calls` and answers with a log.
fn docker<'a>(calls: &'a Calls) -> impl Fn(&[String]) -> Result<Ran, RunError> + 'a {
    move |args| match args.first().map(String::as_str) {
        Some("ps") => ok(PS),
        Some("stats") => ok(STATS),
        _ => {
            calls.borrow_mut().push(args.to_vec());
            Ok(Ran { ok: true, stdout: b"line one\n".to_vec(), stderr: b"warn\n".to_vec() })
        }
    }
}

fn failing(stderr: &'static str) -> impl Fn(&[String]) -> Result<Ran, RunError> {
    move |_| Ok(Ran { ok: false, stdout: Vec::new(), stderr: stderr.into() })
}

fn aimed(command: CommandId, name: &str) -> Run {
    Run::new(command).on(address(name).map(Target::Thing).unwrap_or_default())
}

/// What the app's table holds after `steps` ran against a fresh source.
fn table(steps: impl FnOnce(&FeedTx, &mut Sent) -> Result<(), Closed>) -> Result<Table, Box<dyn Error>> {
    let (tx, inbox) = inbox(|| {});
    steps(&tx.feed(FEED), &mut Sent::new())?;
    let mut model = Model::default();
    inbox.drain(&mut model);
    Ok(model.tables.get(FEED).cloned().unwrap_or_default())
}

#[test]
fn recorded_output_becomes_the_fixtures_rows_grouped_by_project() -> Check {
    let reading = read(PS.as_bytes(), Some(STATS.as_bytes()));
    assert_eq!(reading.note, None);
    let [api, db, registry] = &reading.rows[..] else { return Err("three rows".into()) };
    assert_eq!((api.id.as_str(), db.id.as_str(), registry.id.as_str()), ("atlas-api", "atlas-db", "registry"));
    assert_eq!((api.group.as_deref(), registry.group.as_deref()), (Some("atlas"), Some(OTHER)));
    assert_eq!(api.get("state"), Some(&Value::Dot(Dot::Run)));
    assert_eq!(api.get("cpu"), Some(&Value::Meter { ratio: 0.3352, text: "34%".into() }));
    for (row, field, want) in [
        (api, "mem", "212 MB"), (api, "memory", "212 MB"), (api, "ports", "8080→8080"), (api, "up", "14m"),
        (api, "health", "passing"), (api, "image", "atlas/api:rate-limit"), (api, "group.summary", "compose · 2 running"),
        (db, "mem", "1.9 GB"), (db, "ports", "5432"), (db, "up", "3d"), (db, "group.summary", ""),
        (registry, "up", "exit 137"), (registry, "group.summary", "1 stopped"),
    ] {
        assert_eq!(words(row, field), want, "{} {field}", row.id);
    }
    assert_eq!(registry.get("state"), Some(&Value::Dot(Dot::Fail)));
    assert!(registry.dim);
    for absent in ["cpu", "mem", "ports", "health", "restarts", "last_line"] {
        assert_eq!(registry.get(absent), None, "docker gives no {absent} for an exited container");
    }
    Ok(())
}

#[test]
fn a_bad_line_a_non_ascii_name_and_the_cap_are_each_named_in_the_status() -> Check {
    let ps = format!("{PS}not json\n[1]\n{{\"ID\":\"é\",\"Names\":\"café\",\"Labels\":\"é=ü\"}}\n");
    let reading = read(ps.as_bytes(), Some("{\"ID\":\"é\",\"MemUsage\":\"é\"}\n".as_bytes()));
    assert_eq!(reading.rows.len(), 3);
    assert_eq!(reading.note.as_deref(), Some("3 lines of docker's output were not understood and skipped"));
    let table = table(|out, sent| sent.publish(out, reading))?;
    assert!(matches!(table.status(), FeedStatus::Error(why) if why.starts_with("3 lines")));
    assert_eq!(table.len(), 3, "the rows stay under the note");
    let many: String = (0..MAX_CONTAINERS + 7).map(|n| format!("{{\"ID\":\"{n}\",\"Names\":\"c{n}\"}}\n")).collect();
    let note = read(many.as_bytes(), None).note.unwrap_or_default();
    assert!(note.starts_with("showing 500 of 507 containers (cap 500)"), "{note}");
    assert_eq!(read(many.as_bytes(), None).rows.len(), MAX_CONTAINERS);
    Ok(())
}

#[test]
fn each_way_docker_cannot_answer_is_a_plain_sentence_and_none_is_empty() -> Check {
    let daemon = failing("Cannot connect to the Docker daemon at unix:///x.sock. Is the docker daemon running?");
    let denied = failing("permission denied while trying to connect to the Docker daemon socket");
    let status = |runner: Runner<'_>| table(|out, sent| sent.look(out, runner)).map(|t| t.status().clone());
    assert_eq!(status(&|_| Err(RunError::Missing))?, FeedStatus::Error("Docker is not installed on this Mac".into()));
    assert_eq!(status(&daemon)?, FeedStatus::Error("Docker is not running: start Docker Desktop and this list fills in".into()));
    assert_eq!(status(&denied)?, FeedStatus::Error("Docker refused permission: permission denied while trying to connect to the Docker daemon socket".into()));
    assert_eq!(status(&|_| ok(""))?, FeedStatus::Empty);
    Ok(())
}

#[test]
fn a_reading_after_an_error_is_live_again_but_a_refused_action_outlives_it() -> Check {
    let calls = Calls::default();
    let back = table(|out, sent| { sent.look(out, &docker(&calls))?; sent.look(out, &|_: &[String]| Err(RunError::Missing))?; sent.look(out, &docker(&calls)) })?;
    assert_eq!((back.status(), back.len()), (&FeedStatus::Live, 3));
    let refuses = |args: &[String]| if args.first().is_some_and(|a| a == "rm") { failing("cannot remove a running container")(args) } else { docker(&calls)(args) };
    let table = table(|out, sent| { sent.act(out, &refuses, &aimed(cmd::REMOVE, "atlas-api"))?; sent.look(out, &refuses) })?;
    assert!(matches!(table.status(), FeedStatus::Error(why) if why.contains("cannot remove a running container")), "status after refused rm: {:?}", table.status());
    Ok(())
}

#[test]
fn each_action_runs_exactly_its_docker_arguments_and_a_bad_target_runs_nothing() -> Check {
    let calls = Calls::default();
    table(|out, sent| {
        for command in [cmd::RESTART, cmd::STOP, cmd::START, cmd::REMOVE, cmd::LOGS, cmd::SHELL] {
            sent.act(out, &docker(&calls), &aimed(command, "atlas-api"))?;
        }
        sent.act(out, &docker(&calls), &aimed(cmd::STOP, "a b; rm -rf /"))?;
        sent.act(out, &docker(&calls), &Run::new(cmd::STOP))
    })?;
    let want: [&[&str]; 5] = [&["restart", "atlas-api"], &["stop", "atlas-api"], &["start", "atlas-api"], &["rm", "atlas-api"], &["logs", "--tail", "200", "atlas-api"]];
    let want: Vec<Vec<String>> = want.iter().map(|a| a.iter().map(|s| (*s).to_owned()).collect()).collect();
    assert_eq!(*calls.borrow(), want);
    Ok(())
}

#[test]
fn logs_become_rows_log_name_n_under_their_container_and_a_full_window_says_so() -> Check {
    let calls = Calls::default();
    let table = table(|out, sent| { sent.act(out, &docker(&calls), &aimed(cmd::LOGS, "atlas-api"))?; sent.look(out, &docker(&calls)) })?;
    let line = table.find(&RowId::new("log:atlas-api:2")).ok_or("no second line")?;
    assert_eq!((words(line, "last_line"), line.parent.as_ref().map(RowId::as_str)), ("warn", Some("atlas-api")));
    let full: String = (0..LOG_LINES).map(|n| format!("l{n}\n")).collect();
    let rows = log_rows("db", &full);
    let head = words(&rows[0], "last_line");
    assert!(rows.len() == LOG_LINES + 1 && head.contains("cap --tail 200") && head.contains("docker logs db"), "{head}");
    Ok(())
}

#[test]
fn the_source_reads_again_after_a_command_and_ends_when_the_app_does() -> Check {
    let calls = Calls::default();
    let (tx, _inbox) = inbox(|| {});
    let (cmds, rx) = channel();
    cmds.send(SourceCmd::Run(aimed(cmd::RESTART, "atlas-api")))?;
    drop(cmds);
    serve(&tx.feed(FEED), &rx, &docker(&calls), Duration::from_secs(60))?;
    assert_eq!(calls.borrow().len(), 1);
    Ok(())
}

/// With a 20 ms timer: nothing is read before a pane shows the feed, it is read on the timer
/// while one does, and not again once none does, until Refresh asks.
#[test]
fn the_source_reads_on_its_timer_only_while_a_pane_shows_its_feed() -> Check {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let looks = AtomicUsize::new(0);
    let runner = |args: &[String]| match args.first().map(String::as_str) {
        Some("ps") => { looks.fetch_add(1, SeqCst); ok(PS) }
        _ => ok(STATS),
    };
    let (tx, _inbox) = inbox(|| {});
    let (feed, (cmds, rx)) = (tx.feed(FEED), channel());
    let rest = |ms| std::thread::sleep(Duration::from_millis(ms));
    let until = |n: usize| (0..500).any(|_| { rest(5); looks.load(SeqCst) >= n });
    std::thread::scope(|scope| -> Check {
        let source = scope.spawn(move || serve(&feed, &rx, &runner, Duration::from_millis(20)));
        rest(120);
        assert_eq!(looks.load(SeqCst), 0, "read with no pane showing the feed");
        cmds.send(SourceCmd::Seen(true))?;
        assert!(until(3), "the timer did not run while a pane showed the feed");
        cmds.send(SourceCmd::Seen(false))?;
        rest(80);
        let hidden = looks.load(SeqCst);
        rest(160);
        assert_eq!(looks.load(SeqCst), hidden, "read on its timer after the last pane closed");
        cmds.send(SourceCmd::Refresh)?;
        assert!(until(hidden + 1), "Refresh did not read at once");
        rest(100);
        assert_eq!(looks.load(SeqCst), hidden + 1, "Refresh started the timer again");
        drop(cmds);
        source.join().map_err(|_| "the source died")??;
        Ok(())
    })
}

#[test]
fn ports_and_uptimes_read_the_way_the_mockup_shows_them() {
    assert_eq!(ports("0.0.0.0:3000->3000/tcp, 0.0.0.0:222->22/tcp").as_deref(), Some("3000→3000, 222→22"));
    assert_eq!(ports(""), None);
    let ups = ["Up About an hour", "Up Less than a second", "Up 2 weeks (Paused)", "Exited (0) 3 days ago"];
    assert_eq!(ups.map(uptime), [Some("1h".into()), Some("0s".into()), Some("2w".into()), None]);
}
