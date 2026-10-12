use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Number;
use yi_runtime::fetch::{FETCH_ENTRY_TYPE, FetchError, Page, Resolver};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Wall};
use yi_types::model::{Model, ModelCost};
use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn memory_store() -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "fetch-session-test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

/// Production order: the resolver is wired from the session's store handle
/// before any store exists, and a store attached afterwards serves
/// `history://` and receives the fetch log's own writes.
#[test]
fn a_store_attached_after_the_resolver_still_serves_history() -> TestResult {
    let session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        Arc::new(ProviderStream::new(None)),
    );
    let workspace = Scratch::new("yi-fetch-session")?;
    let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
        .with_session_handle("main", session.store_handle());

    let transcript: Url = "history://main".parse()?;
    let before = resolver
        .fetch(&transcript)
        .err()
        .ok_or("history:// must miss before a store is attached, not serve from a stale handle")?;
    assert!(matches!(before, FetchError::Unsupported { .. }), "{before}");

    let store = memory_store();
    let id = yi_session::lock_session(&store).append_custom("main", "note", None)?;
    session.attach_store(store.clone())?;

    let entry: Url = format!("history://main/{id}").parse()?;
    assert_eq!(resolver.fetch(&entry)?.served_by, "session-entry");
    assert_eq!(resolver.fetch(&transcript)?.served_by, "session-transcript");

    let logged = yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery {
        entry_type: Some("custom"),
        custom_type: Some(FETCH_ENTRY_TYPE.to_owned()),
        ..yi_session::EntryQuery::default()
    })?;
    assert!(
        !logged.is_empty(),
        "the fetch log's own writes must land in the late-attached store"
    );
    Ok(())
}

const PAGED: &str = "plain, then \u{e9}\u{4e16}\u{1f980} wide, then plain again\nline two\n";

fn paged_workspace() -> Result<(Scratch, Resolver, yi_session::SharedSession), Box<dyn Error>> {
    let workspace = Scratch::new("yi-fetch-paged")?;
    std::fs::write(workspace.join("notes.md"), PAGED)?;
    let store = memory_store();
    for _ in 0..5 {
        yi_session::lock_session(&store).append_custom("main", "note", None)?;
    }
    let resolver =
        Resolver::new(workspace.to_path_buf(), Wall::default()).with_session("main", store.clone());
    Ok((workspace, resolver, store))
}

type Read = Result<(String, Option<usize>), Box<dyn Error>>;

fn read(resolver: &Resolver, url: &str, offset: usize, limit: usize) -> Read {
    let fetched = resolver.fetch_page(&url.parse()?, Some(Page { offset, limit }))?;
    Ok((fetched.text, fetched.next_offset))
}

/// Every page of `url` at `limit`, joined by `joint`; refuses a walk that stops advancing.
fn walk(
    resolver: &Resolver,
    url: &str,
    limit: usize,
    joint: &str,
) -> Result<String, Box<dyn Error>> {
    let (mut pages, mut offset) = (Vec::new(), Some(0));
    while let Some(at) = offset {
        let (text, next) = read(resolver, url, at, limit)?;
        if next.is_some_and(|next| next <= at) || pages.len() > PAGED.len() {
            return Err(format!("the walk stalled at {at}: {next:?}").into());
        }
        pages.push(text);
        offset = next;
    }
    Ok(pages.join(joint))
}

#[test]
fn a_capped_read_names_the_next_offset_and_the_next_page_continues_it() -> TestResult {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let file = "local://notes.md";
    assert_eq!(
        read(&resolver, file, 0, 7)?,
        ("plain, ".to_owned(), Some(7))
    );
    assert_eq!(
        read(&resolver, file, 7, usize::MAX)?,
        (PAGED[7..].to_owned(), None)
    );
    // Entries, not bytes. Each fetch logs itself into the same store, so the listing grows
    // under the walk: the pages still rebuild the five notes in order, and the walk ends.
    for url in ["history://main", "history://main/since/0"] {
        let whole = resolver.fetch(&url.parse()?)?.text;
        let notes: Vec<&str> = whole.lines().take(5).collect();
        let (head, next) = read(&resolver, url, 0, 2)?;
        assert_eq!(
            (head.lines().collect::<Vec<_>>(), next),
            (notes[..2].to_vec(), Some(2))
        );
        let walked = walk(&resolver, url, 2, "\n")?;
        assert_eq!(walked.lines().take(5).collect::<Vec<_>>(), notes, "{url}");
    }
    // `tail/N` slides as the log grows, so a second page would repeat or skip an entry.
    assert!(read(&resolver, "history://main/tail/3", 0, 2).is_err());
    Ok(())
}

/// The listing a walk reads is the listing the walk grows, so a page of one entry used to
/// stay one entry behind the end for ever: a paged read of this session's own history
/// appends no log row, and the pages rebuild exactly the listing the walk started from.
#[test]
fn a_walk_of_this_sessions_own_history_ends_at_a_limit_of_one() -> TestResult {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let inbox = "history://self/since/0/custom/note";
    for url in [
        "history://main",
        "history://main/since/0",
        "history://self",
        inbox,
    ] {
        let walked = walk(&resolver, url, 1, "\n")?;
        assert_eq!(resolver.fetch(&url.parse()?)?.text, walked, "{url}");
    }
    let notes = resolver.fetch(&inbox.parse()?)?.text;
    assert_eq!(
        notes.lines().count(),
        5,
        "custom/<type> keeps that type alone: {notes}"
    );
    let one: yi_types::url::Url = "history://self/custom/note/custom/note".parse()?;
    assert!(
        resolver.fetch(&one).is_err(),
        "a filter on a single entry is refused"
    );
    Ok(())
}

/// A byte offset is the caller's arithmetic, so it can land inside a character: the page is
/// still text, the walk still ends, and from a boundary the pages rebuild the file exactly.
#[test]
fn an_offset_inside_a_character_still_serves_text_and_always_advances() -> TestResult {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let file = "local://notes.md";
    for limit in 1..=5 {
        assert_eq!(walk(&resolver, file, limit, "")?, PAGED, "limit {limit}");
        for offset in 0..=PAGED.len() {
            let (text, next) = read(&resolver, file, offset, limit)?;
            assert!(PAGED.contains(&text), "{offset}+{limit}: {text:?}");
            assert!(next.is_none_or(|next| next > offset), "{offset}+{limit}");
            assert_eq!(text.is_empty(), offset == PAGED.len(), "{offset}+{limit}");
        }
    }
    assert_eq!(
        read(&resolver, file, PAGED.len() + 9, 4)?,
        (String::new(), None)
    );
    Ok(())
}

#[test]
fn a_read_that_names_no_page_replies_exactly_as_it_did_before_paging() -> TestResult {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let file: Url = "local://notes.md".parse()?;
    assert_eq!(Page::from_payload(&serde_json::Map::new(), "fetch")?, None);
    let whole = resolver.fetch(&file)?;
    assert_eq!((whole.text.as_str(), whole.next_offset), (PAGED, None));
    let hash = whole.hash.clone();
    assert_eq!(
        serde_json::Value::Object(whole.into_reply(false)),
        serde_json::json!({"url": "local://notes.md", "text": PAGED, "hash": hash, "servedBy": "workspace-file"}),
    );
    let last = resolver.fetch_page(
        &file,
        Page::from_payload(
            &serde_json::Map::from_iter([("offset".to_owned(), 0.into())]),
            "fetch",
        )?,
    )?;
    assert_eq!(
        last.into_reply(true).get("next_offset"),
        Some(&serde_json::Value::Null)
    );
    Ok(())
}

#[test]
fn a_page_the_host_cannot_honour_is_refused_not_clamped() -> TestResult {
    let page = |payload: serde_json::Value| {
        Page::from_payload(payload.as_object().ok_or("an object")?, "fetch")
    };
    for bad in [
        "limit\": 0",
        "limit\": -1",
        "offset\": -1",
        "limit\": 1.5",
        "offset\": \"3\"",
    ] {
        let bad: serde_json::Value = serde_json::from_str(&format!("{{\"{bad}}}"))?;
        assert!(page(bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(
        page(serde_json::json!({"offset": null, "limit": null}))?,
        None
    );
    let rest = Page {
        offset: 3,
        limit: usize::MAX,
    };
    assert_eq!(
        page(serde_json::json!({"offset": 3, "limit": u64::MAX}))?,
        Some(rest)
    );
    assert_eq!(page(serde_json::json!({"offset": 3}))?, Some(rest));
    // Only a listing or a text has pages: one entry and a plan refuse the keys outright.
    let (_workspace, resolver, store) = paged_workspace()?;
    let id = yi_session::lock_session(&store).append_custom("main", "note", None)?;
    for url in [format!("history://main/{id}"), "plan://nothing".to_owned()] {
        let refused = resolver.fetch_page(&url.parse()?, Some(rest));
        assert!(
            matches!(refused, Err(FetchError::BadAddress { .. })),
            "{url}"
        );
    }
    Ok(())
}

/// A program pages an address by feeding `next_offset` back as the tool's 1-based `offset`.
#[test]
fn a_program_pages_an_address_by_feeding_back_its_next_offset() -> Result<(), Box<dyn Error>> {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let resolver = Arc::new(resolver);
    let mut tools: Vec<Arc<dyn yi_tools::Tool>> = (yi_tools::builtin_tools().into_iter())
        .filter(|tool| tool.name() == "read")
        .collect();
    yi_runtime::fetch::route_urls(&mut tools, &resolver);
    let tool = tools.first().ok_or("no read tool")?;
    let mut context = yi_tools::ToolContext::new(std::env::temp_dir());
    context.program = true;
    let page = |offset: u64| -> Result<yi_types::tool_value::FetchedValue, Box<dyn Error>> {
        let input = serde_json::json!({"path": "history://main", "offset": offset, "limit": 1});
        let output = tool.execute(input.as_object().cloned().unwrap_or_default(), &context);
        Ok(serde_json::from_value(output.value.ok_or("no value")?)?)
    };
    let first = page(1)?;
    assert_eq!(first.text, read(&resolver, "history://main", 0, 1)?.0);
    let next = first
        .next_offset
        .ok_or("the listing has more than one entry")?;
    let second = page(next)?;
    assert_eq!(
        second.text,
        read(&resolver, "history://main", 1, 1)?.0,
        "the next page, not the same one"
    );
    Ok(())
}

/// Dies with a url's `offset` and `limit` dropped, its unit unnamed, `limit: 0` read as the
/// rest of the listing, or a negative offset read as none.
#[test]
fn a_url_read_through_the_read_tool_keeps_its_window() -> Result<(), Box<dyn Error>> {
    let (_workspace, resolver, _store) = paged_workspace()?;
    let window = read(&resolver, "history://main", 1, 1)?;
    let resolver = Arc::new(resolver);
    let next = window.1.ok_or("the listing has more than one entry")?;
    assert_eq!(
        read_tool_text(&resolver, "history://main", Some(2), Some(1))?.0,
        format!(
            "{}\n[more from offset {}; offset and limit count entries here]",
            window.0,
            next + 1
        )
    );
    let (refused, is_error) = read_tool_text(&resolver, "history://main", Some(2), Some(0))?;
    assert!(
        is_error && refused.contains("read \"limit\" must be at least 1"),
        "{refused}"
    );
    let (refused, is_error) = read_tool_text(&resolver, "history://main", Some(-5), Some(1))?;
    assert!(
        is_error && refused.contains("read \"offset\" must be a non-negative integer, got -5"),
        "{refused}"
    );
    Ok(())
}

/// Dies with a repository skill linked into a guarded tree served through `yi://skills/`: the
/// address opened the file directly, where every other host-file scheme goes through the read gate.
#[cfg(unix)]
#[test]
fn a_skill_linked_into_a_guarded_tree_is_refused() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-skill-link")?;
    let (workspace, home) = (root.join("repo"), root.join("home"));
    std::fs::create_dir_all(workspace.join(".git"))?;
    std::fs::write(
        workspace.join(".git/config"),
        "[remote \"origin\"]\n\ttoken = s3cret\n",
    )?;
    let skill = workspace.join(".yi/skills/leak");
    std::fs::create_dir_all(&skill)?;
    std::os::unix::fs::symlink(workspace.join(".git/config"), skill.join("SKILL.md"))?;
    std::fs::create_dir_all(&home)?;
    let fetched = Resolver::new(workspace, Wall::default())
        .with_home(home)
        .fetch(&"yi://skills/leak".parse()?);
    assert!(
        matches!(fetched, Err(FetchError::Denied { .. })),
        "{:?}",
        fetched.map(|fetched| fetched.text)
    );
    Ok(())
}

/// A plugin's skill and a file beside it are read through `yi://skills/`, and a path that would
/// climb out of the skill's directory is refused before any file is opened.
#[test]
fn a_plugin_skill_and_its_files_are_read_by_address() -> Result<(), Box<dyn Error>> {
    let root = Scratch::new("yi-skill-files")?;
    let (workspace, home) = (root.join("repo"), root.join("home"));
    std::fs::create_dir_all(&workspace)?;
    let ripwire = yi_tools::plugin::Plugin::named("ripwire").ok_or("no ripwire row")?;
    std::fs::create_dir_all(ripwire.dir(&home))?;
    std::fs::write(ripwire.dir(&home).join("ripwire"), "#!/bin/sh\n")?;
    let orient = ripwire.dir(&home).join("skills/ripwire-orient");
    std::fs::create_dir_all(&orient)?;
    std::fs::write(
        orient.join("SKILL.md"),
        "---\nname: ripwire-orient\n---\nmap first\n",
    )?;
    std::fs::write(orient.join("map-before-you-read.md"), "the ladder\n")?;
    std::fs::write(home.join("secret"), "s3cret\n")?;
    let resolver = Resolver::new(workspace, Wall::default()).with_home(home);
    let read = |address: &str| -> Result<String, Box<dyn Error>> {
        Ok(resolver
            .fetch(&address.parse()?)
            .map(|fetched| fetched.text)
            .map_err(|error| format!("{error:?}"))?)
    };
    assert!(read("yi://skills/ripwire-orient")?.contains("map first"));
    assert!(read("yi://skills/ripwire-orient/map-before-you-read.md")?.contains("the ladder"));
    let climbed = resolver.fetch(&"yi://skills/ripwire-orient/../../../../../secret".parse()?);
    assert!(
        matches!(climbed, Err(FetchError::BadAddress { .. })),
        "{:?}",
        climbed.map(|fetched| fetched.text)
    );
    Ok(())
}

/// A child is told its task, never its parent's session id: `history://parent` reads the parent's
/// transcript off disk as written by the session repo, and a root session has no parent to read.
#[test]
fn a_child_reads_its_parent_transcript_by_alias() -> TestResult {
    use yi_session::SessionRepo;
    let root = Scratch::new("yi-fetch-parent")?;
    let (sessions, cwd) = (root.join("sessions"), root.join("repo"));
    std::fs::create_dir_all(&cwd)?;
    let mut repo = yi_session::JsonlRepo::new(sessions.clone(), cwd.to_string_lossy());
    let parent = repo.create(yi_session::CreateOptions::default())?;
    let parent_id = yi_session::lock_session(&parent).metadata().id.clone();
    let found = yi_ai::faux::faux_assistant_message(
        vec![yi_ai::faux::faux_text(
            "the parent found it in plan/kinds.rs",
        )],
        yi_types::message::StopReason::Stop,
    );
    let asked = yi_session::lock_session(&parent).append_message("main", found)?;
    let child = repo.create(yi_session::CreateOptions {
        parent_session_id: Some(parent_id.clone()),
        ..yi_session::CreateOptions::default()
    })?;
    let host = crate::support::family(root.to_path_buf(), cwd.clone(), child.clone(), None).host;
    let transcripts = Arc::new(yi_runtime::fetch::SessionTranscripts::new(
        host,
        Some(sessions),
        &cwd,
    ));
    let reader = |store: yi_session::SharedSession, wall: Wall| {
        Resolver::new(cwd.clone(), wall)
            .with_session("worker", store)
            .with_transcripts(transcripts.clone())
    };

    let fetched = reader(child.clone(), Wall::default())
        .fetch(&format!("history://parent/{asked}").parse()?)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(fetched.served_by, "session-entry");
    assert!(fetched.text.contains("plan/kinds.rs"), "{}", fetched.text);
    let listing = reader(child.clone(), Wall::default())
        .fetch(&"history://parent".parse()?)
        .map_err(|error| format!("{error:?}"))?;
    assert!(listing.text.contains("plan/kinds.rs"), "{}", listing.text);
    let orphan = reader(parent, Wall::default()).fetch(&"history://parent".parse()?);
    assert!(
        matches!(orphan, Err(FetchError::NotFound { .. })),
        "{:?}",
        orphan.map(|f| f.text)
    );
    let writes = Wall {
        deny_write: vec![cwd.clone()],
        ..Wall::default()
    };
    let reader_child = reader(child.clone(), writes).fetch(&"history://parent/tail/1".parse()?);
    assert!(reader_child.is_ok(), "{:?}", reader_child.err());
    let wall = Wall {
        deny_url: vec!["history://main".to_owned()],
        ..Wall::default()
    };
    let walled = reader(child, wall).fetch(&"history://parent".parse()?);
    assert!(
        matches!(walled, Err(FetchError::Denied { .. })),
        "{:?}",
        walled.map(|f| f.text)
    );
    Ok(())
}

fn read_tool_text(
    resolver: &Arc<Resolver>,
    path: &str,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<(String, bool), Box<dyn Error>> {
    let mut tools: Vec<Arc<dyn yi_tools::Tool>> = yi_tools::builtin_tools()
        .into_iter()
        .filter(|tool| tool.name() == "read")
        .collect();
    yi_runtime::fetch::route_urls(&mut tools, resolver);
    let mut input = serde_json::Map::new();
    input.insert("path".to_owned(), path.into());
    if let Some(offset) = offset {
        input.insert("offset".to_owned(), offset.into());
    }
    if let Some(limit) = limit {
        input.insert("limit".to_owned(), limit.into());
    }
    let output = tools
        .first()
        .ok_or("no read tool")?
        .execute(input, &yi_tools::ToolContext::new(std::env::temp_dir()));
    let text = output
        .result
        .content
        .iter()
        .filter_map(|content| match content {
            yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    Ok((text, output.is_error))
}

/// An unpaged listing is served whole at the 51,200-byte cap and cut one byte past it, with a
/// row naming kept of total, the cap and the offset; a first entry alone over it is clipped on a char.
#[test]
fn an_unpaged_history_listing_past_its_cap_says_where_it_was_cut() -> TestResult {
    const CAP: usize = 50 * 1024;
    let listing = |notes: usize, pad: &str| -> Result<(Resolver, usize), Box<dyn Error>> {
        let store = memory_store();
        for _ in 0..notes {
            yi_session::lock_session(&store).append_custom("main", "note", None)?;
        }
        let data = serde_json::json!({ "pad": pad });
        yi_session::lock_session(&store).append_custom("main", "note", Some(data))?;
        let entries = yi_session::lock_session(&store).find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })?;
        let lines = entries
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        let resolver =
            Resolver::new(std::env::temp_dir(), Wall::default()).with_session("main", store);
        Ok((resolver, lines.join("\n").len()))
    };
    let (_, base) = listing(3, "")?;
    let (at_cap, bytes) = listing(3, &"x".repeat(CAP - base))?;
    assert_eq!(bytes, CAP);
    let whole = read_tool_text(&Arc::new(at_cap), "history://main", None, None)?.0;
    assert!(!whole.contains("[… "), "{}", whole.len());
    let (past, bytes) = listing(3, &"x".repeat(CAP - base + 1))?;
    assert_eq!(bytes, CAP + 1);
    let text = read_tool_text(&Arc::new(past), "history://main", None, None)?.0;
    assert!(
        text.ends_with("[… 3 of 4 entries shown · unpaged history cap 51200 bytes · read history://main with offset=4 and limit to page on]"),
        "{}",
        text.lines().last().unwrap_or_default()
    );
    let (wide, _) = listing(0, &"\u{e9}".repeat(CAP))?;
    let text = read_tool_text(&Arc::new(wide), "history://main", None, None)?.0;
    assert!(
        text.lines()
            .last()
            .is_some_and(|row| row.starts_with("[… 1 of 1 entries shown (the last one clipped)")),
        "{}",
        text.lines().last().unwrap_or_default()
    );
    Ok(())
}

/// A reader's resolver has no session root: its parent is reached live through the host that
/// spawned it, where the on-disk lookup had answered "no session on disk carries that id".
#[test]
fn a_reader_child_reads_its_live_parent_by_alias() -> TestResult {
    let root = Scratch::new("yi-fetch-reader-parent")?;
    let parent = memory_store();
    let said = yi_ai::faux::faux_assistant_message(
        vec![yi_ai::faux::faux_text(
            "the rounding rule is banker's, 2 decimals",
        )],
        yi_types::message::StopReason::Stop,
    );
    yi_session::lock_session(&parent).append_message("main", said)?;
    let parent_id = yi_session::lock_session(&parent).metadata().id.clone();
    let child = Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "reader-child".to_owned(),
            created_at: 0,
            parent_session_id: Some(parent_id),
            name: None,
        },
    )));
    let host = crate::support::family(root.to_path_buf(), root.to_path_buf(), parent, None).host;
    let fetched = Resolver::new(root.to_path_buf(), Wall::default())
        .with_session("reader", child)
        .with_transcripts(Arc::new(yi_runtime::fetch::ParentTranscript(
            Arc::downgrade(&host),
        )))
        .fetch(&"history://parent/since/0".parse()?)
        .map_err(|error| format!("{error:?}"))?;
    assert!(
        fetched.text.contains("banker's, 2 decimals"),
        "{}",
        fetched.text
    );
    Ok(())
}

/// The incident's shape: a user message whose fact sits past 120 chars reads whole in the compact
/// view, across its lines; an assistant line cut at 120 names the call that reads its entry whole.
#[test]
fn a_cut_compact_history_line_names_the_whole_entry_read() -> TestResult {
    let store = memory_store();
    let told = format!(
        "{}\nthe cutover window is Sunday 02:00-04:00 UTC",
        "état des lieux ".repeat(10)
    );
    let user = yi_types::message::AgentMessage::User {
        content: yi_types::message::UserContent::Text(told),
        timestamp: 0,
        attribution: yi_types::message::Attribution::User,
    };
    yi_session::lock_session(&store).append_message("main", user)?;
    let resolver =
        || Resolver::new(std::env::temp_dir(), Wall::default()).with_session("main", store.clone());
    let plain = read_tool_text(&Arc::new(resolver()), "history://main/since/0", None, None)?.0;
    assert!(
        plain.contains(" ⏎ the cutover window is Sunday 02:00-04:00 UTC"),
        "{plain}"
    );
    assert!(!plain.contains("[… a line ending in"), "{plain}");
    let said = yi_ai::faux::faux_assistant_message(
        vec![yi_ai::faux::faux_text(&"état des lieux ".repeat(10))],
        yi_types::message::StopReason::Stop,
    );
    yi_session::lock_session(&store).append_message("main", said)?;
    let compact = read_tool_text(&Arc::new(resolver()), "history://main/since/0", None, None)?.0;
    assert_eq!(
        compact.lines().last(),
        Some(
            "[… a line is cut at 120 chars (NOTE_CHARS), a user line at 2000 (USER_CHARS) · a cut line carries its whole entry as history://<agent>/<entry-id>]"
        )
    );
    let cut_line = compact
        .lines()
        .find(|line| line.contains(" · whole: history://main/"))
        .ok_or("no cut line carries its whole entry")?;
    let whole = cut_line
        .rsplit(" · whole: ")
        .next()
        .ok_or("the cut line carries no pointer")?;
    let entry = read_tool_text(&Arc::new(resolver()), whole, None, None)?.0;
    assert!(entry.contains("état des lieux "), "{entry}");
    assert!(!entry.contains("history://parent/since/0"), "{entry}");
    Ok(())
}

/// A compact line that legitimately ends in … is not announced as cut: the cut is carried by
/// the view that made it, never sniffed out of the rendered text.
#[test]
fn a_legitimate_ellipsis_in_a_compact_line_is_not_a_cut_row() -> TestResult {
    let store = memory_store();
    let user = yi_types::message::AgentMessage::User {
        content: yi_types::message::UserContent::Text("it ends with an ellipsis …".to_owned()),
        timestamp: 0,
        attribution: yi_types::message::Attribution::User,
    };
    yi_session::lock_session(&store).append_message("main", user)?;
    let text = read_tool_text(
        &Arc::new(Resolver::new(std::env::temp_dir(), Wall::default()).with_session("main", store)),
        "history://main/since/0",
        None,
        None,
    )?
    .0;
    assert!(text.contains("it ends with an ellipsis …"), "{text}");
    assert!(!text.contains("[… a line ending in"), "{text}");
    Ok(())
}

/// The cap proof covers the `since/` window too, through the same read-tool route as the bare
/// listing: both arms the `matches!` names are pinned by a red run, not green by omission.
#[test]
fn a_since_window_past_the_cap_says_where_it_was_cut() -> TestResult {
    let store = memory_store();
    for _ in 0..40 {
        let user = yi_types::message::AgentMessage::User {
            content: yi_types::message::UserContent::Text("x".repeat(2100)),
            timestamp: 0,
            attribution: yi_types::message::Attribution::User,
        };
        yi_session::lock_session(&store).append_message("main", user)?;
    }
    let text = read_tool_text(
        &Arc::new(Resolver::new(std::env::temp_dir(), Wall::default()).with_session("main", store)),
        "history://main/since/0",
        None,
        None,
    )?
    .0;
    let row = text.lines().last().unwrap_or_default();
    assert!(row.starts_with("[… "), "{row}");
    assert!(row.contains("entries shown"), "{row}");
    assert!(row.contains("unpaged history cap 51200 bytes"), "{row}");
    Ok(())
}

/// A wall that denies history refuses a family member's live transcript by `agent://` too, the
/// other spelling of the same transcript.
#[test]
fn a_history_wall_refuses_a_members_agent_address() -> TestResult {
    let root = Scratch::new("yi-fetch-agent-walled")?;
    let wall = Wall {
        deny_url: vec!["history://main".to_owned()],
        ..Wall::default()
    };
    let fetched = Resolver::new(root.to_path_buf(), wall).fetch(&"agent://sibling".parse()?);
    assert!(
        matches!(fetched, Err(FetchError::Denied { .. })),
        "{:?}",
        fetched.map(|f| f.text)
    );
    Ok(())
}
