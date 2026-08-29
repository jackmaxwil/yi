use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::json;

use super::{Effect, Event, EventMask, Extension, Rank, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    OneShot,
    Complex,
    Undecided,
}

impl Route {
    fn label(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Complex => "complex",
            Self::Undecided => "undecided",
        }
    }
}

pub const IMPERATIVES: [&str; 12] = [
    "audit",
    "decompose",
    "implement",
    "migrate",
    "port",
    "redesign",
    "refactor",
    "rework",
    "rewrite",
    "split",
    "standardize",
    "unify",
];

pub const QUESTIONS: [&str; 7] = ["explain", "how", "what", "when", "where", "which", "why"];

fn word_hits(prompt: &str, table: &[&str]) -> i32 {
    let mut hits = 0;
    for word in prompt.split(|ch: char| !ch.is_ascii_alphabetic()) {
        if word.is_empty() {
            continue;
        }
        let word = word.to_ascii_lowercase();
        if table.binary_search(&word.as_str()).is_ok() {
            hits += 1;
        }
    }
    hits
}

fn enumerations(prompt: &str) -> i32 {
    let count = prompt
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            ["- ", "* ", "+ "]
                .iter()
                .any(|bullet| line.starts_with(*bullet))
                || line
                    .split_once(['.', ')'])
                    .is_some_and(|(head, _)| !head.is_empty() && head.chars().all(char::is_numeric))
        })
        .count();
    i32::try_from(count).unwrap_or(i32::MAX).min(4)
}

pub fn prefilter(prompt: &str, repo_dirty: bool, named_paths: u32) -> Route {
    let words = prompt.split_whitespace().count();
    let imperatives = word_hits(prompt, &IMPERATIVES);
    let paths = i32::try_from(named_paths).unwrap_or(i32::MAX);
    let mut score = 0i32;
    // Short and quiet: no verb of scale, no path named, nothing changed yet.
    // A short prompt that names three subsystems is not quiet.
    if words < 12 && !repo_dirty && imperatives == 0 && named_paths == 0 {
        score -= 3;
    }
    if prompt.contains("```") {
        score -= 1;
    }
    score += i32::try_from(prompt.matches(" and ").count())
        .unwrap_or(i32::MAX)
        .min(3);
    score += enumerations(prompt);
    score += imperatives;
    score -= word_hits(prompt, &QUESTIONS);
    score += paths.saturating_sub(1).max(0);
    match score {
        s if s <= -3 => Route::OneShot,
        s if s >= 4 => Route::Complex,
        _ => Route::Undecided,
    }
}

const TOOL_CALLS_PER_TURN: u32 = 4;
const FILES_MATCHED: u32 = 5;

pub struct Orchestrate {
    fragment: &'static str,
    attached: bool,
    reads: BTreeSet<PathBuf>,
    edited: bool,
}

impl Orchestrate {
    pub fn new(fragment: &'static str) -> Self {
        Self {
            fragment,
            attached: false,
            reads: BTreeSet::new(),
            edited: false,
        }
    }

    fn attach(&mut self, out: &mut Vec<Effect>, signal: &'static str) {
        if self.attached {
            return;
        }
        self.attached = true;
        out.push(Effect::AttachFragment {
            slot: Slot::new(Rank::Protocol, "orchestrate"),
            text: self.fragment.to_owned(),
        });
        out.push(Effect::Record {
            key: "orchestrate_attached",
            value: json!({ "signal": signal }),
        });
        if signal != "prefilter" {
            out.push(Effect::Remind {
                text: "This task has outgrown one-shot handling; write the plan now.".to_owned(),
            });
        }
    }

    fn on_prompt(
        &mut self,
        prompt: &str,
        repo_dirty: bool,
        named_paths: u32,
        out: &mut Vec<Effect>,
    ) {
        let route = if mentions_orchestration(prompt) {
            Route::Complex
        } else {
            prefilter(prompt, repo_dirty, named_paths)
        };
        out.push(Effect::Record {
            key: "route",
            value: json!({
                "route": route.label(),
                "words": prompt.split_whitespace().count(),
                "repo_dirty": repo_dirty,
                "named_paths": named_paths,
            }),
        });
        if route == Route::Complex {
            self.attach(out, "prefilter");
        }
    }

    fn on_tool_call(&mut self, name: &str, target: Option<&PathBuf>, out: &mut Vec<Effect>) {
        match name {
            "read" => {
                if let Some(target) = target {
                    self.reads.insert(target.clone());
                }
            }
            "edit" => {
                self.edited = true;
                if target.is_some_and(|path| !self.reads.contains(path)) {
                    self.attach(out, "edit_before_read");
                }
            }
            "write" => self.edited = true,
            _ => {}
        }
    }
}

fn mentions_orchestration(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    ["orchestrate", "write a plan", "create a plan", "plan this"]
        .iter()
        .any(|needle| lower.contains(needle))
}

impl Extension for Orchestrate {
    fn name(&self) -> &'static str {
        "orchestrate"
    }

    fn interests(&self) -> EventMask {
        EventMask::PROMPT
            .with(EventMask::TOOL_CALL)
            .with(EventMask::TOOL_RESULT)
            .with(EventMask::TURN_END)
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        match event {
            Event::PromptSubmitted {
                prompt,
                repo_dirty,
                named_paths,
            } => self.on_prompt(prompt, *repo_dirty, *named_paths, out),
            Event::ToolCall { name, target, .. } => self.on_tool_call(name, target.as_ref(), out),
            Event::ToolResult {
                name,
                exit,
                files_matched,
                ..
            } => {
                if *files_matched > FILES_MATCHED {
                    self.attach(out, "files_matched");
                }
                if self.edited && name == "bash" && exit.is_some_and(|code| code != 0) {
                    self.attach(out, "failed_check_after_edit");
                }
            }
            Event::TurnEnd {
                tool_calls_this_turn,
                ..
            } => {
                if *tool_calls_this_turn > TOOL_CALLS_PER_TURN {
                    self.attach(out, "tool_calls_per_turn");
                }
            }
            _ => {}
        }
    }
}
