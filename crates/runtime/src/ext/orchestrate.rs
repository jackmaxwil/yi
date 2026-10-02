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

/// Invariant: the persisted route row reads these fields, so the scorer and the telemetry
/// cannot disagree; a Python mirror would be a shadow model that drifts from the constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    pub words: usize,
    pub enums: i32,
    pub imperatives: i32,
    pub questions: i32,
    pub and_count: i32,
    pub fenced: bool,
    pub score: i32,
}

impl Features {
    pub fn route(self) -> Route {
        match self.score {
            s if s <= crate::levers::get().route_oneshot_at => Route::OneShot,
            s if s >= crate::levers::get().route_complex_at => Route::Complex,
            _ => Route::Undecided,
        }
    }
}

pub fn features(prompt: &str, repo_dirty: bool, named_paths: u32) -> Features {
    let words = prompt.split_whitespace().count();
    let imperatives = word_hits(prompt, &IMPERATIVES);
    let questions = word_hits(prompt, &QUESTIONS);
    let enums = enumerations(prompt);
    let fenced = prompt.contains("```");
    let and_count = i32::try_from(prompt.matches(" and ").count())
        .unwrap_or(i32::MAX)
        .min(3);
    let paths = i32::try_from(named_paths).unwrap_or(i32::MAX);
    let mut score = 0i32;
    // Short and quiet: no verb of scale, no path named, nothing changed yet.
    // A short prompt that names three subsystems is not quiet.
    if words < 12 && !repo_dirty && imperatives == 0 && named_paths == 0 {
        score -= 3;
    }
    if fenced {
        score -= 1;
    }
    score += and_count;
    score += enums;
    score += imperatives;
    score -= questions;
    score += paths.saturating_sub(1).max(0);
    Features {
        words,
        enums,
        imperatives,
        questions,
        and_count,
        fenced,
        score,
    }
}

pub fn prefilter(prompt: &str, repo_dirty: bool, named_paths: u32) -> Route {
    features(prompt, repo_dirty, named_paths).route()
}

pub const COMPLEX_AT: i32 = 4;
pub const ONESHOT_AT: i32 = -3;
pub const TOOL_CALLS_PER_TURN: u32 = 4;
pub const FILES_MATCHED: u32 = 5;

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

    fn attach(&mut self, out: &mut Vec<Effect>, signal: &'static str, remind: bool) {
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
        if remind {
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
        let features = features(prompt, repo_dirty, named_paths);
        let route = if mentions_orchestration(prompt) {
            Route::Complex
        } else {
            features.route()
        };
        out.push(Effect::Record {
            key: "route",
            value: json!({
                "route": route.label(),
                "words": features.words,
                "repo_dirty": repo_dirty,
                "named_paths": named_paths,
                "score": features.score,
                "enums": features.enums,
                "imperatives": features.imperatives,
                "questions": features.questions,
                "and_count": features.and_count,
                "fenced": features.fenced,
            }),
        });
        if route == Route::Complex {
            self.attach(out, "prefilter", false);
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
                    self.attach(out, "edit_before_read", true);
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
                if *files_matched > crate::levers::get().route_files_matched {
                    self.attach(out, "files_matched", false);
                }
                if self.edited && name == "bash" && exit.is_some_and(|code| code != 0) {
                    self.attach(out, "failed_check_after_edit", true);
                }
            }
            Event::TurnEnd {
                tool_calls_this_turn,
                ..
            } => {
                // Incident: twenty reads and no write nudged "write the plan now" after the
                // answer had shipped; the model took the nudge for the user and burned a turn.
                if *tool_calls_this_turn > crate::levers::get().route_tool_calls_per_turn
                    && self.edited
                {
                    self.attach(out, "tool_calls_per_turn", false);
                }
            }
            _ => {}
        }
    }
}
