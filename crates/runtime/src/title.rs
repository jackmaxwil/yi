use yi_loop::interrupt::InterruptSignal;
use yi_session::lock_session;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::model::LlmContext;

use crate::AgentSession;

/// The card request from omp `title-fork.md`: the model picks the emoji and the
/// code for this session. The parked-car example is the line shape, not the icon.
const ASK: &str = "Name this session for a small card: an icon, a short code, then a 3-6 word \
     title for the user's goal, informed by the user's request and your reasoning so far.\n\
     - emoji: the most specific emoji picture of this session's subject, one that tells it \
     apart from the user's other sessions in the same project: the concrete object or feature \
     the title names (a tree for a directory walker, a muted bell for muting notifications); a \
     mascot or symbol whenever the session is about that language, tool or service (a whale \
     for Docker builds), never just because the project is written in it; a bug, a test tube \
     or a bolt only when the session is about that very thing, never as a stand-in for the \
     kind of work (any fix, test or speedup); never a generic sparkle, rocket, wrench, gear \
     or puzzle piece.\n\
     - code: 1-6 capital letters or digits, never more, that name the subject at a glance, \
     made from one word or name in the title: the whole word when it has 6 letters or fewer \
     (SEED, SCOPE, FREEZE, Z3, 404), else cut to its first syllable or leading consonants \
     (PROV for provider, FLBK for fallback). Never initials of several words unless the user \
     writes them so (CTA, SDK), nor a code that passes for an unrelated name.\n\
     - Title: imperative phrase built from the user's own key words: the feature, component, \
     or error they name. A short question is still a task.\n\
     - No words to name? Answer <title/>.\n\
     - No quotes, trailing punctuation, or explanation.\n\
     Output exactly one line: <title emoji=\"🅿️\" code=\"PARK\">Fix flaky park tests</title> \
     (your own emoji, code and title), or <title/>.";

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
        cache_ttl: yi_types::model::Ttl::Min5,
        cache_tail: true,
        system_prompt: "You name coding sessions.".to_owned(),
        messages: vec![AgentMessage::task(&prompt, 0)],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::OneShot,
        tools: None,
        tool_choice: None,
    };
    let (reply, usage) = crate::compaction::complete_text(
        session.provider(),
        &model,
        &context,
        model.clamp_effort(yi_types::model::Effort::Off),
        &InterruptSignal::default(),
    )
    .await;
    // A lost cost row must not cost the session its title.
    crate::spend::book_side_call(&store, "side:title", usage).ok();
    let text = reply?;
    if declined(&text) {
        return Ok(None);
    }
    let Some(title) = named(&text) else {
        return Err(format!("the summarizer's title was empty: {text:?}"));
    };
    lock_session(&store)
        .set_name(Some(title.clone()))
        .map_err(|error| error.to_string())?;
    Ok(Some(title))
}

/// `<title emoji="🧪" code="FLAKY">Fix flaky park tests</title>` becomes `🧪 FLAKY: Fix flaky
/// park tests`; unparsed `<title` markup is never the name, plain text is.
pub fn named(text: &str) -> Option<String> {
    match title_tag(text) {
        Some((_, "")) => None,
        Some((attrs, body)) => {
            card(attr(attrs, "emoji"), attr(attrs, "code"), body).or_else(|| clean(body))
        }
        None if opens_title(text) => None,
        None => clean(text),
    }
}

/// The model had nothing to name: `<title/>`, a card with no body, or broken `<title` markup.
fn declined(text: &str) -> bool {
    opens_title(text) && named(text).is_none()
}

fn opens_title(text: &str) -> bool {
    text.to_ascii_lowercase().contains("<title")
}

fn card(emoji: Option<&str>, code: Option<&str>, body: &str) -> Option<String> {
    let icon = emoji.map(str::trim).filter(|icon| is_icon(icon))?;
    let code = code
        .map(|code| code.trim().to_ascii_uppercase())
        .filter(|code| {
            !code.is_empty() && code.len() <= 6 && code.chars().all(|c| c.is_ascii_alphanumeric())
        })?;
    let title = clean(body)?;
    Some(format!("{icon} {code}: {title}"))
}

/// Invariant: an icon is 1-8 non-ASCII characters and none is invisible, so a model cannot
/// reorder or hide the tab text; U+200D joins emoji and stays.
fn is_icon(icon: &str) -> bool {
    let invisible = |ch: char| {
        ch.is_control()
            || matches!(ch, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200c}' | '\u{200e}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
    };
    let count = icon.chars().count();
    (1..=8).contains(&count) && icon.chars().all(|ch| !ch.is_ascii() && !invisible(ch))
}

fn title_tag(text: &str) -> Option<(&str, &str)> {
    let start = text.to_ascii_lowercase().find("<title")?;
    let rest = text.get(start + "<title".len()..)?;
    let (opening, body) = rest.split_once('>')?;
    let attrs = opening.trim().trim_end_matches('/').trim();
    if opening.trim_end().ends_with('/') {
        return Some((attrs, ""));
    }
    let close = body.to_ascii_lowercase().find("</title>")?;
    Some((attrs, body.get(..close)?.trim()))
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=");
    let at = attrs.to_ascii_lowercase().find(&key)?;
    let rest = attrs.get(at + key.len()..)?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = rest.get(quote.len_utf8()..)?;
    let stop = inner.find(quote)?;
    inner.get(..stop)
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

#[cfg(test)]
mod tests {
    use super::{declined, named};

    #[test]
    fn a_card_reply_keeps_the_emoji_the_model_chose() {
        assert_eq!(
            named("<title emoji=\"🧪\" code=\"flaky\">Fix flaky park tests.</title>"),
            Some("🧪 FLAKY: Fix flaky park tests".to_owned())
        );
        assert_eq!(
            named("<title emoji=\"🌲\" code=\"TREE\">Walk the directory</title>"),
            Some("🌲 TREE: Walk the directory".to_owned())
        );
        assert!(declined("<title/>"));
        assert_eq!(named("<title/>"), None);
        assert_eq!(
            named("\"Fix the console context gauge.\""),
            Some("Fix the console context gauge".to_owned())
        );
    }

    #[test]
    fn markup_that_is_not_a_card_never_becomes_the_name() {
        for reply in [
            "<title emoji=\"🧪\" code=\"X\"/>",
            "<title emoji=\"🧪\" code=\"FLAKY\">Fix flaky park tests",
            "Sure: <TITLE emoji=\"🧪\" code=\"X\"></title>",
        ] {
            assert!(declined(reply), "{reply}");
            assert_eq!(named(reply), None, "{reply}");
        }
    }

    #[test]
    fn an_invisible_character_cannot_ride_in_the_emoji() {
        let card = |icon: &str| {
            named(&format!(
                "<title emoji=\"{icon}\" code=\"RTL\">Fix the tab</title>"
            ))
        };
        assert_eq!(card("\u{202e}🧪"), Some("Fix the tab".to_owned()));
        assert_eq!(card("🧪\u{200b}"), Some("Fix the tab".to_owned()));
        assert_eq!(card("é"), Some("é RTL: Fix the tab".to_owned()));
        assert_eq!(
            card("👨\u{200d}👩\u{200d}👧"),
            Some("👨\u{200d}👩\u{200d}👧 RTL: Fix the tab".to_owned())
        );
    }
}
