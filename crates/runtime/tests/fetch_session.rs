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
    assert_eq!(Page::from_payload(&serde_json::Map::new())?, None);
    let whole = resolver.fetch(&file)?;
    assert_eq!((whole.text.as_str(), whole.next_offset), (PAGED, None));
    let hash = whole.hash.clone();
    assert_eq!(
        serde_json::Value::Object(whole.into_reply(false)),
        serde_json::json!({"url": "local://notes.md", "text": PAGED, "hash": hash, "servedBy": "workspace-file"}),
    );
    let last = resolver.fetch_page(
        &file,
        Page::from_payload(&serde_json::Map::from_iter([(
            "offset".to_owned(),
            0.into(),
        )]))?,
    )?;
    assert_eq!(
        last.into_reply(true).get("next_offset"),
        Some(&serde_json::Value::Null)
    );
    Ok(())
}

#[test]
fn a_page_the_host_cannot_honour_is_refused_not_clamped() -> TestResult {
    let page =
        |payload: serde_json::Value| Page::from_payload(payload.as_object().ok_or("an object")?);
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

/// Dies with `offset` and `limit` dropped for a url (the whole `history://` listing comes back),
/// with the page's unit unnamed, or with `limit: 0` read as the rest of the listing.
#[test]
fn a_url_read_through_the_read_tool_keeps_its_window() -> Result<(), Box<dyn Error>> {
    let (workspace, resolver, _store) = paged_workspace()?;
    let window = read(&resolver, "history://main", 1, 1)?;
    let mut tools: Vec<Arc<dyn yi_tools::Tool>> = yi_tools::builtin_tools()
        .into_iter()
        .filter(|tool| tool.name() == "read")
        .collect();
    yi_runtime::fetch::route_urls(&mut tools, &Arc::new(resolver));
    let tool = tools.first().ok_or("no read tool")?;
    let mut input = serde_json::Map::new();
    input.insert("path".to_owned(), "history://main".into());
    input.insert("offset".to_owned(), 2.into());
    input.insert("limit".to_owned(), 1.into());
    let text_of = |input| {
        let output = tool.execute(input, &yi_tools::ToolContext::new(workspace.to_path_buf()));
        let text: String = output
            .result
            .content
            .iter()
            .filter_map(|content| match content {
                yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        (text, output.is_error)
    };
    let next = window.1.ok_or("the listing has more than one entry")?;
    assert_eq!(
        text_of(input.clone()).0,
        format!(
            "{}\n[more from offset {}; offset and limit count entries here]",
            window.0,
            next + 1
        )
    );
    input.insert("limit".to_owned(), 0.into());
    let (refused, is_error) = text_of(input);
    assert!(
        is_error && refused.contains("\"limit\" must be at least 1"),
        "{refused}"
    );
    Ok(())
}
