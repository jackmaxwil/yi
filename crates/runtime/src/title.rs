use yi_loop::interrupt::InterruptSignal;
use yi_session::lock_session;
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
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
    let Some(title) = clean(&text) else {
        return Err(format!("the summarizer's title was empty: {text:?}"));
    };
    lock_session(&store)
        .set_name(Some(title.clone()))
        .map_err(|error| error.to_string())?;
    Ok(Some(title))
}

const TITLE_CAP: usize = 48;

pub fn clean(text: &str) -> Option<String> {
    clean_within(text, TITLE_CAP)
}

fn clean_within(text: &str, max: usize) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line
        .trim_start_matches(['#', '*', ' '])
        .trim_matches(['"', '\'', '`', '*', ' '])
        .trim_end_matches('.');
    let mut title = String::new();
    for word in line.split_whitespace() {
        let word: String = word.chars().filter(|&ch| !invisible(ch)).collect();
        if word.is_empty() {
            continue;
        }
        if title.chars().count() + word.chars().count() + 1 > max {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(&word);
    }
    (!title.is_empty()).then_some(title)
}

/// Invariant: a title never carries a character that reorders or hides text; controls are
/// the sink's to drop.
fn invisible(ch: char) -> bool {
    matches!(ch, '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{2800}' | '\u{3164}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{e0000}'..='\u{e007f}')
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn a_reply_is_its_first_non_empty_line_without_wrapping_marks_or_its_period() {
        assert_eq!(
            clean("\n\n## Fix the park tests.\nA second paragraph"),
            Some("Fix the park tests".to_owned())
        );
        assert_eq!(
            clean("\"Fix the console context gauge.\""),
            Some("Fix the console context gauge".to_owned())
        );
        assert_eq!(clean("`Fix the tab`"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("**Fix the tab**"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("'Fix the tab'"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("Fix the tab"), Some("Fix the tab".to_owned()));
    }

    #[test]
    fn a_plain_title_that_mentions_the_tag_is_still_a_title() {
        let plain = "Fix <title> tag rendering";
        assert_eq!(clean(plain), Some(plain.to_owned()));
        assert_eq!(
            clean("Fix the </title> closing tag"),
            Some("Fix the </title> closing tag".to_owned())
        );
    }

    #[test]
    fn the_title_loses_bidi_overrides_and_hidden_characters() {
        assert_eq!(clean("Fix \u{202e}the tab"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("Fix\u{200b} the tab"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("\u{2800}Fix the tab"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("\u{e0041}🧪 Fix"), Some("🧪 Fix".to_owned()));
        assert_eq!(clean("Fix the\u{2060} tab"), Some("Fix the tab".to_owned()));
        assert_eq!(clean("\u{feff}"), None);
    }

    #[test]
    fn the_title_stays_within_the_cap_and_loses_no_whole_word() {
        let long = "word ".repeat(30);
        let name = clean(&long).unwrap_or_default();
        assert!((1..=48).contains(&name.chars().count()), "{name}");
        assert!(name.split(' ').all(|word| word == "word"), "{name}");
        let nine = "abcd ".repeat(10);
        let cut = clean(&nine).unwrap_or_default();
        assert_eq!(cut.chars().count(), 44);
        assert_eq!(cut.split(' ').count(), 9);
        let fifteen = "123456789012345";
        let wide = format!("{fifteen} {fifteen} {fifteen} {fifteen} {fifteen}");
        let kept = clean(&wide).unwrap_or_default();
        assert_eq!(kept, format!("{fifteen} {fifteen} {fifteen}"));
    }

    #[test]
    fn the_cap_counts_characters_so_a_multibyte_title_is_never_sliced() {
        let icons = "🧪 ".repeat(30);
        let name = clean(&icons).unwrap_or_default();
        assert!((1..=48).contains(&name.chars().count()), "{name}");
        assert!(name.chars().all(|ch| ch == '🧪' || ch == ' '), "{name}");
        assert!(name.ends_with('🧪'), "{name}");
        assert!(name.is_char_boundary(name.len()), "{name}");
    }

    #[test]
    fn a_reply_with_no_word_to_name_is_no_title() {
        assert_eq!(clean(""), None);
        assert_eq!(clean("   \n\t "), None);
        assert_eq!(clean("..."), None);
        assert_eq!(clean("\"\" "), None);
        assert_eq!(clean("#"), None);
    }
}
