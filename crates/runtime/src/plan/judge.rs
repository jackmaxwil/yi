//! The judge tier (plan section 6.4): a judged contract item is decided by a jury of walled
//! reader children of another model family, each answering one fixed schema, tallied here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use yi_types::fetch::FetchRecord;
use yi_types::model::Model;
use yi_types::plan::canonical::ArtifactRef;
use yi_types::plan::contract::{
    ContractItem, Decider, ItemVerdict, JurorAnswer, JurorLine, Quote, Vote,
};

use super::verify::{Judge, Seat, Snapshot};
use crate::subagent::models::{family_of, other_families, selector_of};
use crate::subagent::{ChildExit, SubagentHost};

pub const JUDGE_BRIEF: &str = include_str!("../prompts/judge.md");

/// The lease one juror draws: reserved against the owner's grant and returned when it retires.
const JUROR_TOKENS: u64 = 100_000;
const POLL: Duration = Duration::from_millis(50);
const REASON_CHARS: usize = 240;

/// Every scheme but `local://`: the evidence is workspace files and a juror reads nothing else.
/// Cooperative, as every wall is; the quote check is what refuses a source that is not evidence.
const DENY_URL: [&str; 11] = [
    "history://",
    "kernel://",
    "tree://",
    "family://",
    "agent://",
    "plan://",
    "user://",
    "mcp://",
    "checkpoint://",
    "http://",
    "https://",
];

/// The models a juror may be seated on; the host's registry, never a payload.
pub type Registry = dyn Fn() -> Vec<Model> + Send + Sync;

pub struct Jury {
    host: Arc<SubagentHost>,
    registry: Arc<Registry>,
}

/// One evidence artifact as a juror is told to read it and as `local://` serves it.
pub struct Evidence {
    pub url: String,
    text: String,
    hash: String,
}

impl Evidence {
    pub fn served(url: String, raw: &str) -> Self {
        let (text, hash) = crate::fetch::as_served(raw);
        Self { url, text, hash }
    }

    /// Backed when this juror's own log holds a whole read of the address at the frozen hash
    /// and the quoted line is that line. A blank line decides nothing, so it backs nothing.
    fn backs(&self, quote: &Quote, rows: &[FetchRecord]) -> bool {
        !quote.text.trim().is_empty()
            && quote.url == self.url
            && rows
                .iter()
                .any(|row| row.url == self.url && row.hash == self.hash)
            && quote
                .line
                .checked_sub(1)
                .and_then(|index| self.text.lines().nth(index))
                .is_some_and(|line| line.trim() == quote.text.trim())
    }
}

fn clip(text: &str) -> String {
    text.chars().take(REASON_CHARS).collect()
}

/// Invariant: every answer that is not the schema is an abstention, and a decided vote that
/// quotes nothing is one too. `Err` is an unbacked quote, which abstains the whole item.
pub fn vote_of(
    answer: &str,
    rows: &[FetchRecord],
    evidence: &[Evidence],
) -> Result<(Vote, String), String> {
    let Ok(answer) = serde_json::from_str::<JurorAnswer>(answer.trim()) else {
        let reason = "the answer was not the verdict schema".to_owned();
        return Ok((Vote::Abstain, reason));
    };
    for quote in &answer.quotes {
        if !evidence.iter().any(|source| source.backs(quote, rows)) {
            return Err(format!(
                "unbacked quote: {} line {}",
                clip(&quote.url),
                quote.line
            ));
        }
    }
    if answer.verdict != Vote::Abstain && answer.quotes.is_empty() {
        let reason = "a decided vote quoted no evidence".to_owned();
        return Ok((Vote::Abstain, reason));
    }
    Ok((answer.verdict, clip(&answer.reason)))
}

/// The predeclared quorum: a lone judge decides alone, and of three, two like votes decide.
/// The quorum is over the seats asked for, so an abstention or an empty seat never shrinks it.
pub fn tally(lines: &[JurorLine], n: u8) -> ItemVerdict {
    let count = |vote: Vote| lines.iter().filter(|line| line.vote == vote).count();
    let quorum = usize::from(n / 2).saturating_add(1);
    if count(Vote::Pass) >= quorum {
        return ItemVerdict::Pass;
    }
    if count(Vote::Fail) >= quorum {
        let reasons: Vec<&str> = lines
            .iter()
            .filter(|line| line.vote == Vote::Fail)
            .map(|line| line.reason.as_str())
            .collect();
        return ItemVerdict::Fail {
            detail: reasons.join("; "),
        };
    }
    ItemVerdict::Abstain {
        reason: format!(
            "no quorum of {quorum} in a jury of {n}: {} pass, {} fail",
            count(Vote::Pass),
            count(Vote::Fail)
        ),
    }
}

/// The rubric, the evidence addresses and the schema, and nothing else: no transcript, no
/// author, no earlier verdict, and the evidence itself only through the juror's own fetch.
pub fn brief(rubric: &str, evidence: &[Evidence]) -> String {
    let urls: Vec<String> = evidence
        .iter()
        .map(|source| format!("- {}", source.url))
        .collect();
    format!(
        "{JUDGE_BRIEF}\n<rubric>\n{}\n</rubric>\n\nEvidence:\n{}\n",
        rubric.trim(),
        urls.join("\n")
    )
}

impl Jury {
    /// Seats jurors on the registry's models a provider key is set for, and on the host's
    /// own provider, whose credentials evidently work.
    pub fn new(host: Arc<SubagentHost>) -> Self {
        let defaults = Arc::clone(&host.options.defaults);
        let registry = move || {
            let own = defaults().0.provider;
            let mut models = crate::provider::available_models();
            models.retain(|model| {
                model.provider == own || yi_ai::auth::api_key(&model.provider).is_some()
            });
            models
        };
        Self::over(host, Arc::new(registry))
    }

    pub fn over(host: Arc<SubagentHost>, registry: Arc<Registry>) -> Self {
        Self { host, registry }
    }

    /// Invariant: the owner's family is read from the host's own settings and the model the
    /// plan spawned the todo's child with, and a juror of either is never seated.
    fn jurors(&self, seat: &Seat<'_>, n: u8) -> Result<Vec<Model>, String> {
        let mut owners = vec![(self.host.options.defaults)().0];
        if let Some(selector) = seat.owner_model {
            let named = selector
                .split_once('/')
                .and_then(|(provider, id)| crate::provider::resolve_model(provider, id))
                .ok_or("the owner's model is not in the registry")?;
            owners.push(named);
        }
        let owners: Vec<&str> = owners.iter().map(family_of).collect();
        let others = other_families((self.registry)(), &owners);
        if others.is_empty() {
            return Err("no other family".to_owned());
        }
        Ok(others.into_iter().cycle().take(usize::from(n)).collect())
    }

    fn evidence(
        &self,
        snapshot: &Snapshot<'_>,
        sources: &[ArtifactRef],
    ) -> Result<Vec<Evidence>, String> {
        if sources.is_empty() {
            return Err("the item names no evidence".to_owned());
        }
        sources
            .iter()
            .map(|source| {
                let path = snapshot.artifacts.path(&source.digest);
                let relative = path
                    .strip_prefix(&self.host.options.cwd)
                    .map_err(|_| "the evidence is outside the workspace a juror reads")?;
                let bytes = snapshot
                    .artifacts
                    .get(&source.digest)
                    .map_err(|error| error.to_string())?;
                let raw = String::from_utf8(bytes).map_err(|_| "the evidence is not text")?;
                let url = format!("local://{}", relative.display());
                Ok(Evidence::served(url, &raw))
            })
            .collect()
    }

    /// One seated juror's vote (`Err` is an unbacked quote), read at its end or at `until`.
    /// Retired, never reaped: its answer is the tally's, not the transcript's of the owner judged.
    fn vote(
        &self,
        name: &str,
        evidence: &[Evidence],
        until: Instant,
    ) -> Result<(Vote, String), String> {
        let live = |host: &SubagentHost| {
            host.children.lock().is_ok_and(|children| {
                children
                    .values()
                    .any(|record| record.session_name == name && record.exit.is_none())
            })
        };
        while live(&self.host) && Instant::now() < until {
            std::thread::sleep(POLL);
        }
        let record = match self.host.retire(name) {
            Ok((_key, record, _settled)) => record,
            Err(reason) => return Ok((Vote::Abstain, clip(&reason))),
        };
        if record.exit != Some(ChildExit::Completed) {
            let verb = crate::family::read_exit(record.exit).verb;
            return Ok((Vote::Abstain, format!("the juror {verb}")));
        }
        let answer =
            crate::subagent::last_assistant_text(&record.session.messages()).unwrap_or_default();
        let rows = record.session.store();
        let rows = rows.map(|store| crate::fetch::rows_of(&store));
        vote_of(&answer, &rows.unwrap_or_default(), evidence)
    }

    fn sit(
        &self,
        item: &ContractItem,
        snapshot: &Snapshot<'_>,
        until: Instant,
    ) -> Result<(ItemVerdict, Vec<JurorLine>), String> {
        let Decider::Judge {
            rubric,
            evidence,
            policy,
        } = &item.decider
        else {
            return Err("not a judge item".to_owned());
        };
        let seat = snapshot.jury.as_ref().ok_or("no jury sits on this path")?;
        let models = self.jurors(seat, policy.n)?;
        let evidence = self.evidence(snapshot, evidence)?;
        let rubric = snapshot.artifacts.get(&rubric.digest);
        let rubric = rubric.map_err(|error| error.to_string())?;
        let prompt = brief(&String::from_utf8_lossy(&rubric), &evidence);
        let suffix = crate::subagent::random_suffix()?;
        // All seats are spawned before any is read, so the jurors run side by side.
        let seats: Vec<(String, Result<(), String>)> = models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let name = format!("judge-{}-{suffix}-{index}", item.id);
                let kwargs = json!({"name": name, "role": "root", "model": selector_of(model), "tokens": JUROR_TOKENS,
                    "deny_write": ["."], "deny_url": DENY_URL});
                let kwargs = kwargs.as_object().cloned().unwrap_or_default();
                let spawned = self
                    .host
                    .spawn_seated(prompt.clone(), kwargs, Some(seat.permit));
                (name, spawned.map(|_handle| ()))
            })
            .collect();
        let mut unbacked = None;
        let lines: Vec<JurorLine> = seats
            .into_iter()
            .zip(&models)
            .map(|((name, spawned), model)| {
                let read = match spawned {
                    Ok(()) => self.vote(&name, &evidence, until),
                    Err(refusal) => Ok((Vote::Abstain, clip(&refusal))),
                };
                let fabricated = read.is_err();
                let (vote, reason) = read.unwrap_or_else(|quote| {
                    unbacked = Some(quote.clone());
                    (Vote::Abstain, quote)
                });
                JurorLine {
                    model: selector_of(model),
                    vote,
                    reason,
                    unbacked: fabricated,
                }
            })
            .collect();
        let verdict = match unbacked {
            Some(reason) => ItemVerdict::Abstain { reason },
            None => tally(&lines, policy.n),
        };
        Ok((verdict, lines))
    }
}

impl Judge for Jury {
    fn judge(
        &self,
        item: &ContractItem,
        snapshot: &Snapshot<'_>,
        until: Instant,
    ) -> (ItemVerdict, Vec<JurorLine>) {
        self.sit(item, snapshot, until)
            .unwrap_or_else(|reason| (ItemVerdict::Abstain { reason }, Vec::new()))
    }
}
