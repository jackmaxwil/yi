pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

pub const SUMMARIZATION_PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Next Steps
1. [Ordered list of what should happen next]

Keep each section concise. Constraints, progress, decisions and file pointers are already in the compact view."#;

pub const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE existing Goal items, add new ones if the task expanded
- UPDATE Next Steps based on what was accomplished
- Constraints, progress, decisions and file pointers live in the compact view, not here

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Next Steps
1. [Update based on current state]

Keep each section concise."#;

pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix."#;

pub const KERNEL_PERSIST_SUMMARY_NOTE: &str = "Note: the IPython kernel keeps running after this summary — every Python variable, import, and helper you defined stays available. The cells that defined them won't appear above, so record in the summary any names worth remembering so you reuse them instead of redefining them.";

pub fn build_summarization_prompt(
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
) -> String {
    let mut prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_owned()
    } else {
        SUMMARIZATION_PROMPT.to_owned()
    };
    if let Some(instructions) = custom_instructions {
        prompt.push_str(&format!(
            "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above: emphasize what they ask to focus on, and preserve verbatim anything they ask to remember.\n{instructions}\n</user-instructions>"
        ));
    }
    prompt
}
