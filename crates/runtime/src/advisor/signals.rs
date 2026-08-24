use serde_json::Value;
use yi_types::message::{AgentMessage, Content};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    NoOpEditRepeat,
    ToolFailureStreak,
    PreIrreversible,
    UnbackedClaim,
    RepeatTool,
    VerificationSkip,
}

impl SignalKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::NoOpEditRepeat => "no_op_edit_repeat",
            Self::ToolFailureStreak => "tool_failure_streak",
            Self::PreIrreversible => "pre_irreversible",
            Self::UnbackedClaim => "unbacked_claim",
            Self::RepeatTool => "repeat_tool",
            Self::VerificationSkip => "verification_skip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fired {
    pub kind: SignalKind,
    pub evidence: Vec<String>,
}

/// §7.4 verb table — one table, two uses: claim extraction here, sentence
/// selection in the digest.
pub const CLAIM_VERBS: [&str; 10] = [
    "ran",
    "tested",
    "verified",
    "edited",
    "created",
    "fixed",
    "passes",
    "passed",
    "all green",
    "green",
];
pub const COMMITMENT_VERBS: [&str; 4] = ["will", "next", "then", "instead"];
pub const CONCLUSION_VERBS: [&str; 3] = ["because", "so", "root cause"];

// Bookkeeping tools are transparent to loop detection (TraceProbe): a read
// between two identical greps still counts as a loop.
const VERIFICATION_COMMANDS: [&str; 8] = [
    "cargo test",
    "cargo build",
    "just check",
    "pytest",
    "npm test",
    "make test",
    "go test",
    "cargo check",
];

pub type IrreversibleProbe = dyn Fn(&str, &serde_json::Map<String, Value>) -> bool + Send + Sync;

#[derive(Default)]
struct TurnLog {
    tool_calls: Vec<(String, String)>,
    edits: bool,
    verification_ran: bool,
}

/// The six launch signals (design §7.3), stateful over the message stream.
pub struct Signals {
    consecutive_failures: u64,
    repeat: Option<(String, String, u64)>,
    turn: TurnLog,
    irreversible: Option<std::sync::Arc<IrreversibleProbe>>,
}

impl Signals {
    pub fn new(irreversible: Option<std::sync::Arc<IrreversibleProbe>>) -> Self {
        Self {
            consecutive_failures: 0,
            repeat: None,
            turn: TurnLog::default(),
            irreversible,
        }
    }

    pub fn observe(&mut self, message: &AgentMessage) -> Vec<Fired> {
        let mut fired = Vec::new();
        match message {
            AgentMessage::ToolResult {
                tool_name,
                is_error,
                ..
            } => {
                if *is_error {
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    if self.consecutive_failures == 3 {
                        fired.push(Fired {
                            kind: SignalKind::ToolFailureStreak,
                            evidence: vec![format!(
                                "3 consecutive failed tool calls, last: {tool_name}"
                            )],
                        });
                    }
                } else {
                    self.consecutive_failures = 0;
                }
            }
            AgentMessage::Assistant {
                content,
                stop_reason,
                ..
            } => {
                let mut called_tool = false;
                for block in content {
                    if let Content::ToolCall {
                        name, arguments, ..
                    } = block
                    {
                        called_tool = true;
                        fired.extend(self.observe_tool_call(name, arguments));
                    }
                }
                // A final answer (no tool calls) closes the turn: check
                // verification_skip and unbacked claims against the turn log.
                if !called_tool && *stop_reason == yi_types::message::StopReason::Stop {
                    let text = assistant_text(content);
                    if self.turn.edits && !self.turn.verification_ran && !text.is_empty() {
                        fired.push(Fired {
                            kind: SignalKind::VerificationSkip,
                            evidence: vec![
                                "edits landed and a final answer was emitted with no test/build command in the turn"
                                    .to_owned(),
                            ],
                        });
                    }
                    fired.extend(self.unbacked_claims(&text));
                    self.turn = TurnLog::default();
                }
            }
            AgentMessage::User { .. } => {
                self.turn = TurnLog::default();
                self.consecutive_failures = 0;
                self.repeat = None;
            }
            _ => {}
        }
        fired
    }

    fn observe_tool_call(
        &mut self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
    ) -> Vec<Fired> {
        let mut fired = Vec::new();
        let canonical = serde_json::to_string(arguments).unwrap_or_default();
        self.turn
            .tool_calls
            .push((name.to_owned(), canonical.clone()));
        if matches!(name, "edit" | "write") {
            self.turn.edits = true;
        }
        if matches!(name, "bash" | "ipython") {
            let command = arguments
                .get("command")
                .or_else(|| arguments.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if VERIFICATION_COMMANDS
                .iter()
                .any(|probe| command.contains(probe))
            {
                self.turn.verification_ran = true;
            }
        }
        match &mut self.repeat {
            Some((last_name, last_args, count))
                if *last_name == name && *last_args == canonical =>
            {
                *count = count.saturating_add(1);
                if *count == 3 {
                    let kind = if name == "edit" {
                        SignalKind::NoOpEditRepeat
                    } else {
                        SignalKind::RepeatTool
                    };
                    fired.push(Fired {
                        kind,
                        evidence: vec![format!("{name} called 3 times with identical arguments")],
                    });
                }
            }
            _ => self.repeat = Some((name.to_owned(), canonical, 1)),
        }
        if let Some(probe) = &self.irreversible
            && probe(name, arguments)
        {
            fired.push(Fired {
                kind: SignalKind::PreIrreversible,
                evidence: vec![format!("pending {name} call is irreversible")],
            });
        }
        fired
    }

    /// §7.4: claim sentences (action verb + file/command/test object) with no
    /// matching tool call in the turn log. High recall, low precision — a
    /// trigger, not a verdict.
    fn unbacked_claims(&self, text: &str) -> Vec<Fired> {
        let mut fired = Vec::new();
        for sentence in split_sentences(text) {
            let lowered = sentence.to_lowercase();
            let claims = CLAIM_VERBS.iter().any(|verb| {
                lowered
                    .split_whitespace()
                    .any(|word| word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric()) == *verb)
                    || lowered.contains("all green")
            });
            if !claims {
                continue;
            }
            let backed = self.turn.tool_calls.iter().any(|(name, args)| {
                matches!(name.as_str(), "edit" | "write" | "bash" | "ipython")
                    && sentence_mentions_args(&lowered, args)
            }) || (lowered.contains("test") && self.turn.verification_ran)
                || (self.turn.edits && (lowered.contains("edit") || lowered.contains("fixed")));
            if !backed {
                fired.push(Fired {
                    kind: SignalKind::UnbackedClaim,
                    evidence: vec![sentence.trim().to_owned()],
                });
            }
        }
        fired
    }
}

fn sentence_mentions_args(sentence: &str, canonical_args: &str) -> bool {
    canonical_args
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '/' && ch != '_')
        .filter(|token| token.len() > 3)
        .any(|token| sentence.contains(&token.to_lowercase()))
}

pub fn assistant_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn split_sentences(text: &str) -> Vec<&str> {
    text.split_inclusive(['.', '!', '?', '\n'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}
