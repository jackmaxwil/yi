use yi_loop::interrupt::InterruptSignal;
use yi_session::lock_session;
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::LlmContext;

use crate::AgentSession;

const ASK: &str = "Name this coding session in three to six words: a plain title, no quotes, no \
     trailing period. Reply with the title alone.";

pub async fn title_session(session: &AgentSession) -> Result<Option<String>, String> {
    let Some(store) = session.store() else {
        return Ok(None);
    };
    let model = session.summarizer();
    if lock_session(&store).name().is_some() {
        return Ok(None);
    }
    let entries = lock_session(&store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .map_err(|error| error.to_string())?;
    let messages: Vec<&AgentMessage> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    let asked = messages.iter().find_map(|message| match message {
        AgentMessage::User { .. } => Some(message.plain_text()),
        _ => None,
    });
    let Some(asked) = asked.filter(|text| !text.trim().is_empty()) else {
        return Ok(None);
    };
    let answered = messages
        .iter()
        .find_map(|message| match message {
            AgentMessage::Assistant { .. } => Some(message.plain_text()),
            _ => None,
        })
        .unwrap_or_default();
    let clip = |text: &str| text.chars().take(1500).collect::<String>();
    let prompt = format!(
        "{ASK}\n\nThe user asked:\n{}\n\nThe agent answered:\n{}",
        clip(&asked),
        clip(&answered)
    );
    let context = LlmContext {
        system_prompt: "You name coding sessions.".to_owned(),
        messages: yi_context::convert_to_llm(&[AgentMessage::host_user(
            UserContent::Text(prompt),
            0,
        )]),
        tools: None,
        tool_choice: None,
    };
    let text = crate::compaction::complete_text(
        session.provider(),
        &model,
        &context,
        &InterruptSignal::default(),
    )
    .await?;
    let Some(title) = clean(&text) else {
        return Err(format!("the summarizer's title was empty: {text:?}"));
    };
    lock_session(&store)
        .set_name(Some(title.clone()))
        .map_err(|error| error.to_string())?;
    Ok(Some(title))
}

pub fn clean(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line
        .trim_start_matches(['#', '*', ' '])
        .trim_matches(['"', '\'', '`', '*', ' '])
        .trim_end_matches('.');
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut title = String::new();
    for word in words {
        if title.chars().count() + word.chars().count() + 1 > 48 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    (!title.is_empty()).then_some(title)
}
