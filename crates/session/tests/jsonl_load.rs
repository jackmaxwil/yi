use std::error::Error;
use std::fs;
use std::path::PathBuf;

use yi_session::{EntryQuery, JsonlRepo, LogOptions, SessionRepo, load_session};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn golden_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../types/tests/fixtures/v4-golden.jsonl")
}

#[test]
fn loads_a_pi_generated_v4_session_file() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("golden.jsonl");
    fs::copy(golden_fixture(), &path)?;
    let store = load_session(&path)?;

    assert_eq!(store.metadata().id, "fixture-a");
    let entries = store.find_entries(&EntryQuery::default())?;
    assert!(!entries.is_empty());
    let log = store.log(&LogOptions::default())?;
    assert_eq!(log.len(), 40);
    assert!(store.lanes().iter().any(|pointer| pointer.lane == "thread"));
    assert!(store.stats().message_count > 0);
    Ok(())
}

#[test]
fn repairs_a_torn_tail_by_dropping_the_partial_line() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("torn.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"torn","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\n{entry}\n{{\"kind\":\"ent"))?;

    let store = load_session(&path)?;
    assert_eq!(store.find_entries(&EntryQuery::default())?.len(), 1);
    let repaired = fs::read_to_string(&path)?;
    assert_eq!(repaired, format!("{header}\n{entry}\n"));
    Ok(())
}

/// Dies with one shared staging name: two loads repairing the same torn file wrote one temp
/// file, and the rename that lost found it gone.
#[test]
fn loads_repairing_one_torn_file_at_once_all_succeed() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("golden-torn.jsonl");
    let mut torn = fs::read_to_string(golden_fixture())?;
    torn.push_str("{\"kind\":\"ent");
    fs::write(&path, &torn)?;
    let loads: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || load_session(&path).map(|store| store.metadata().id.clone()))
        })
        .collect();
    for load in loads {
        let id = load.join().map_err(|_| "a load panicked")??;
        assert_eq!(id, "fixture-a");
    }
    assert_eq!(
        fs::read_to_string(&path)?,
        fs::read_to_string(golden_fixture())?
    );
    Ok(())
}

#[test]
fn reterminates_a_file_missing_its_trailing_newline() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("unterminated.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"unterminated","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\n{entry}"))?;

    let store = load_session(&path)?;
    assert_eq!(store.find_entries(&EntryQuery::default())?.len(), 1);
    let repaired = fs::read_to_string(&path)?;
    assert!(repaired.ends_with('\n'));
    Ok(())
}

#[test]
fn rejects_a_mid_file_corrupt_line() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("corrupt.jsonl");
    let header = r#"{"kind":"header","version":4,"id":"corrupt","createdAt":1,"cwd":"/tmp"}"#;
    let entry = r#"{"kind":"entry","lane":"main","type":"custom","id":"e1","customType":"note","parentId":null,"seq":1,"timestamp":1}"#;
    fs::write(&path, format!("{header}\nnot json\n{entry}\n"))?;

    match load_session(&path) {
        Ok(_) => Err("expected invalid_entry for mid-file corruption".into()),
        Err(error) => {
            assert_eq!(error.code(), "invalid_entry");
            Ok(())
        }
    }
}

/// A listing names a session from its first prompt, and a name fact written later wins.
#[test]
fn list_names_sessions_from_the_first_prompt_or_the_name_fact() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let mut repo = JsonlRepo::new(dir.to_path_buf(), "/tmp/yi-named");
    let header = |id: &str, at: u64| {
        format!(
            r#"{{"kind":"header","version":4,"id":"{id}","createdAt":{at},"cwd":"/tmp/yi-named"}}"#
        )
    };
    let user = r#"{"kind":"entry","lane":"main","type":"message","id":"e1","message":{"role":"user","content":"  fix the login bug\nand the logout one","timestamp":0},"parentId":null,"seq":1,"timestamp":1}"#;
    let fact = r#"{"kind":"fact","seq":2,"fact":"name","name":"login work"}"#;
    let sessions = dir.join("--tmp-yi-named--");
    fs::create_dir_all(&sessions)?;
    fs::write(
        sessions.join("1_first.jsonl"),
        format!("{}\n{user}\n", header("first", 1)),
    )?;
    fs::write(
        sessions.join("2_named.jsonl"),
        format!("{}\n{user}\n{fact}\n", header("named", 2)),
    )?;
    fs::write(
        sessions.join("3_bare.jsonl"),
        format!("{}\n", header("bare", 3)),
    )?;
    let listed = repo.list()?;
    let names: Vec<(String, Option<String>)> = listed
        .iter()
        .map(|metadata| (metadata.id.clone(), metadata.name.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("bare".to_owned(), None),
            ("named".to_owned(), Some("login work".to_owned())),
            ("first".to_owned(), Some("fix the login bug".to_owned())),
        ]
    );
    Ok(())
}

/// A long file parses in parallel chunks yet replays in order, repairs a torn tail, and names
/// the true line of a corrupt one.
#[test]
fn a_long_session_loads_in_order_across_parse_chunks() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let path = dir.join("long.jsonl");
    let mut body =
        r#"{"kind":"header","version":4,"id":"long","createdAt":1,"cwd":"/tmp"}"#.to_owned();
    let mut parent = "null".to_owned();
    for seq in 1..=3000 {
        body.push_str(&format!(
            "\n{{\"kind\":\"entry\",\"lane\":\"main\",\"type\":\"custom\",\"id\":\"e{seq}\",\"customType\":\"note\",\"parentId\":{parent},\"seq\":{seq},\"timestamp\":1}}"
        ));
        parent = format!("\"e{seq}\"");
    }
    fs::write(&path, format!("{body}\n{{\"kind\":\"ent"))?;
    let store = load_session(&path)?;
    let entries = store.find_entries(&EntryQuery {
        order: yi_session::EntryOrder::OldestFirst,
        ..EntryQuery::default()
    })?;
    let ids: Vec<String> = entries.iter().map(|entry| entry.id().to_owned()).collect();
    let expected: Vec<String> = (1..=3000).map(|seq| format!("e{seq}")).collect();
    assert_eq!(ids, expected);
    assert_eq!(fs::read_to_string(&path)?, format!("{body}\n"));

    let corrupt = body.replacen("\"id\":\"e2500\"", "\"id\":", 1);
    fs::write(&path, format!("{corrupt}\n"))?;
    let error = load_session(&path)
        .err()
        .ok_or("a corrupt line must fail")?;
    assert!(error.to_string().contains("line 2501"), "{error}");
    Ok(())
}

/// The list index reads only what changed: a grown file from its tail, a shrunk one whole, a
/// deleted one drops out, and a corrupt index is rebuilt from the files.
#[test]
fn the_list_index_follows_appends_shrinks_deletes_and_corruption() -> TestResult {
    let dir = Scratch::new("yi-session-jsonl")?;
    let mut repo = JsonlRepo::new(dir.to_path_buf(), "/tmp/yi-indexed");
    let sessions = dir.join("--tmp-yi-indexed--");
    fs::create_dir_all(&sessions)?;
    let header = |id: &str, at: u64| {
        format!(
            r#"{{"kind":"header","version":4,"id":"{id}","createdAt":{at},"cwd":"/tmp/yi-indexed"}}"#
        )
    };
    let user = |text: &str| {
        format!(
            r#"{{"kind":"entry","lane":"main","type":"message","id":"e1","message":{{"role":"user","content":"{text}","timestamp":0}},"parentId":null,"seq":1,"timestamp":1}}"#
        )
    };
    let fact = r#"{"kind":"fact","seq":2,"fact":"name","name":"renamed"}"#;
    let (first, second) = (sessions.join("1_a.jsonl"), sessions.join("2_b.jsonl"));
    fs::write(
        &first,
        format!("{}\n{}\n", header("a", 1), user("fix login")),
    )?;
    fs::write(&second, format!("{}\n", header("b", 2)))?;
    fs::write(sessions.join("notes.jsonl"), "{\"not\":\"a session\"}\n")?;
    type Names = Vec<(String, Option<String>)>;
    let mut names = || -> Result<Names, Box<dyn Error>> {
        Ok(repo
            .list()?
            .into_iter()
            .map(|metadata| (metadata.id, metadata.name))
            .collect())
    };
    let named = |pairs: &[(&str, Option<&str>)]| -> Names {
        pairs
            .iter()
            .map(|(id, name)| ((*id).to_owned(), name.map(str::to_owned)))
            .collect()
    };
    assert_eq!(names()?, named(&[("b", None), ("a", Some("fix login"))]));
    assert!(
        sessions.join(".index.json").exists(),
        "the first list writes the index"
    );

    let append = |path: &std::path::Path, line: &str| -> std::io::Result<()> {
        use std::io::Write;
        writeln!(fs::OpenOptions::new().append(true).open(path)?, "{line}")
    };
    append(&first, fact)?;
    append(&second, &user("second prompt"))?;
    assert_eq!(
        names()?,
        named(&[("b", Some("second prompt")), ("a", Some("renamed"))]),
        "grown files are read from their tails"
    );

    fs::write(
        &first,
        format!("{}\n{}\n", header("a", 1), user("fix login")),
    )?;
    fs::remove_file(&second)?;
    assert_eq!(
        names()?,
        named(&[("a", Some("fix login"))]),
        "a shrunk file is reread"
    );
    let index = fs::read_to_string(sessions.join(".index.json"))?;
    assert!(
        !index.contains("2_b.jsonl"),
        "a deleted file leaves the index"
    );

    fs::write(sessions.join(".index.json"), "not json")?;
    assert_eq!(
        names()?,
        named(&[("a", Some("fix login"))]),
        "a corrupt index is rebuilt"
    );
    Ok(())
}
