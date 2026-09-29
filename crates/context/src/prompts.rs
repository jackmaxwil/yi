pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

pub const SUMMARIZATION_PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks. Cite each by the address of the user message that asked for it, from <user-messages> (e.g. user://3), and quote the user's words verbatim in quotation marks if you quote them at all; never restate them in your own words.]

## Constraints & Preferences
- [Each constraint, preference, or requirement the user stated, cited by the address of the message that stated it; verbatim in quotation marks when quoted]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

pub const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals with their user:// addresses, add new ones if the task expanded, each cited by the address of the user message that asked for it; the user's words are quoted verbatim or not at all]

## Constraints & Preferences
- [Preserve existing with their addresses, add new ones discovered, each cited by its user:// address; verbatim when quoted]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

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
    prompt.push_str("\n\n");
    prompt.push_str(KERNEL_PERSIST_SUMMARY_NOTE);
    // The request carries the session's tools so its prefix stays the loop's (§7).
    prompt.push_str("\n\nReply with the summary as text and call no tool.");
    prompt
}
