//! The findings a thread has, read from todos, goal discoveries and claims, with the user's
//! triage laid over them, and the counts and gate the hub draws.

use std::collections::BTreeMap;

use yi_types::goal::{Goal, GoalStatus};
use yi_types::plan::contract::Resolution;
use yi_types::plan::doc::{Todo, TodoState};
use yi_types::todo::{Claim, RowCheck};
use yi_types::url::Scheme;

use crate::address::thing::Thing;
use crate::address::uri::{Address, AddressError};
use crate::findings::state::{Event, Mark, Proof, Stage, walk};
use crate::findings::triage::{Decision, FindingId, Triage};
use crate::ids::{AgentId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::reduce::threads::Thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    High,
    Med,
    Low,
}

/// The record a finding is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Todo,
    Discovery,
    Claim,
    /// Made by the user with `finding.new`.
    You,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// The command, or what would have to hold.
    pub text: String,
    pub mark: Mark,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub id: FindingId,
    pub thread: ThreadId,
    pub title: String,
    pub severity: Severity,
    pub stage: Stage,
    pub origin: Origin,
    /// Where it points: a file, else the thread's folder.
    pub place: Address,
    pub owner: Option<AgentId>,
    /// How long ago it was raised; none when the server records no time for it.
    pub age: Option<Millis>,
    pub evidence: Vec<Thing>,
    pub checks: Vec<Check>,
}

impl Finding {
    /// `yi://finding/<id>`.
    pub fn address(&self) -> Result<Address, AddressError> {
        Address::new(Kind::Finding, self.id.as_str())
    }

    /// The finding as a model reads it: its title, severity and stage, where it points, its
    /// evidence and checks, every place a repository path rather than an app address.
    pub fn words(&self) -> String {
        let mut said = format!(
            "{}\n{:?} · {:?} · {}",
            self.title,
            self.severity,
            self.stage,
            path(&self.place)
        );
        for each in &self.evidence {
            said.push_str(&format!(
                "\nEvidence: {} ({})",
                each.label,
                path(&each.address)
            ));
        }
        for check in &self.checks {
            said.push_str(&format!("\nCheck: {} ({:?})", check.text, check.mark));
        }
        said
    }
}

/// A place as a path and range, `src/lib.rs#L3`; a kind with no path keeps its id.
fn path(address: &Address) -> String {
    match address.part_text() {
        Some(part) => [address.id(), "#", part].concat(),
        None => address.id().to_owned(),
    }
}

/// The finding at `id` in any thread the model knows.
pub fn find(model: &Model, id: &FindingId) -> Option<Finding> {
    let threads = model.wire.threads.keys();
    let mut all = threads.flat_map(|thread| findings(model, thread, Scope::Thread));
    all.find(|finding| &finding.id == id)
}

/// Which threads a view reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Thread,
    /// Every thread whose folder is the shown thread's.
    Project,
}

/// A finding before triage and the table decide its stage.
struct Raw {
    id: FindingId,
    title: String,
    severity: Severity,
    origin: Origin,
    place: Address,
    owner: Option<AgentId>,
    age: Option<Millis>,
    evidence: Vec<Address>,
    checks: Vec<Check>,
    /// What the record says happened, in order.
    events: Vec<Event>,
}

/// The findings of `shown`, or of its whole project, oldest record first. Dismissed and
/// merged findings do not show; a merged one's evidence shows under the one it joined.
pub fn findings(model: &Model, shown: &ThreadId, scope: Scope) -> Vec<Finding> {
    let threads = &model.wire.threads;
    let folder = threads.get(shown).map(|thread| thread.cwd.as_str());
    let picked = threads.values().filter(|thread| match (scope, folder) {
        (Scope::Project, Some(folder)) if !folder.is_empty() => thread.cwd == folder,
        (Scope::Thread | Scope::Project, _) => &thread.id == shown,
    });
    let mut raws: Vec<(ThreadId, Raw)> = Vec::new();
    for thread in picked {
        raws.extend(
            of_thread(model, thread)
                .into_iter()
                .map(|raw| (thread.id.clone(), raw)),
        );
    }
    raws.extend(made(model, shown));
    let triage = &model.findings.triage;
    let mut joined: BTreeMap<FindingId, Vec<Address>> = BTreeMap::new();
    for (_, raw) in &raws {
        if let Some(Decision::Merged { into }) = triage.decision(&raw.id) {
            joined
                .entry(into.clone())
                .or_default()
                .extend(raw.evidence.iter().cloned());
        }
    }
    raws.into_iter()
        .filter_map(|(thread, raw)| settle(model, triage, &mut joined, thread, raw))
        .collect()
}

fn settle(
    model: &Model,
    triage: &Triage,
    joined: &mut BTreeMap<FindingId, Vec<Address>>,
    thread: ThreadId,
    mut raw: Raw,
) -> Option<Finding> {
    let accepted = match triage.decision(&raw.id) {
        Some(Decision::Dismissed { .. } | Decision::Merged { .. }) => return None,
        Some(Decision::Accepted) => true,
        None => false,
    };
    raw.evidence
        .extend(joined.remove(&raw.id).unwrap_or_default());
    let events = accepted
        .then_some(Event::Accept)
        .into_iter()
        .chain(raw.events);
    let proof = Proof::of(raw.checks.iter().map(|check| check.mark)).map(Event::Pass);
    let failed = raw.checks.iter().any(|check| check.mark == Mark::Failing);
    let verdict = proof.or(failed.then_some(Event::Fail));
    let stage = walk(Stage::New, events.chain(verdict));
    Some(Finding {
        id: raw.id,
        thread,
        title: raw.title,
        severity: raw.severity,
        stage,
        origin: raw.origin,
        place: raw.place,
        owner: raw.owner,
        age: raw.age,
        evidence: raw
            .evidence
            .into_iter()
            .map(|address| Thing::at(model, address))
            .collect(),
        checks: raw.checks,
    })
}

fn of_thread(model: &Model, thread: &Thread) -> Vec<Raw> {
    let work = &thread.work;
    let mut out = Vec::new();
    let mut stack: Vec<&Todo> = work
        .todos
        .iter()
        .flat_map(|list| list.phases.iter().flat_map(|phase| phase.items.iter()))
        .rev()
        .collect();
    while let Some(todo) = stack.pop() {
        out.extend(todo_raw(model, thread, todo));
        stack.extend(todo.children.iter().rev());
    }
    let labels: Vec<&str> = out.iter().map(|raw| raw.title.as_str()).collect();
    let orphans: Vec<&Claim> = work
        .claims
        .iter()
        .filter(|claim| !labels.contains(&claim.label.as_str()))
        .collect();
    out.extend(
        orphans
            .into_iter()
            .filter_map(|claim| claim_raw(thread, claim)),
    );
    if let Some(goal) = &work.goal {
        out.extend(goal.discoveries.iter().filter_map(|found| {
            let id = FindingId::new(format!("{}/discovery/{}", thread.id, found.fingerprint));
            let severity = match found.violates_check_of {
                Some(_) => Severity::High,
                None => Severity::Med,
            };
            Some(Raw {
                id,
                title: found.text.clone(),
                severity,
                origin: Origin::Discovery,
                place: home(thread)?,
                owner: None,
                age: None,
                evidence: Vec::new(),
                checks: goal_check(goal).into_iter().collect(),
                events: Vec::new(),
            })
        }));
    }
    out
}

fn todo_raw(model: &Model, thread: &Thread, todo: &Todo) -> Option<Raw> {
    let label = todo.label.as_str();
    let handle = todo.id.as_ref().map_or(label, |id| id.as_str());
    let id = FindingId::new(format!("{}/todo/{handle}", thread.id));
    let mut checks = Vec::new();
    let mut evidence = Vec::new();
    if let Some(check) = RowCheck::of(todo) {
        let mark = if check.last.is_green() {
            Mark::Met
        } else if check.last.digest().is_some() {
            Mark::Failing
        } else if asked(model, thread, &check.command) {
            Mark::Waiting
        } else {
            Mark::Pending
        };
        checks.push(Check {
            text: check.command,
            mark,
        });
    }
    let mut events = Vec::new();
    let mut owner = None;
    let mut place = None;
    match &todo.state {
        TodoState::Pending | TodoState::Other(_) => {}
        TodoState::Abandoned => return None,
        TodoState::Running { by } => {
            owner = Some(AgentId::new(by.as_str()));
            events.push(Event::Assign);
        }
        TodoState::Blocked { .. } => events.push(Event::Assign),
        TodoState::Failed { cause, .. } => {
            events.push(Event::Assign);
            checks.push(Check {
                text: cause.clone(),
                mark: Mark::Failing,
            });
        }
        TodoState::Done { output, resolution } => {
            events.extend([Event::Assign, Event::Propose]);
            if let Some(quoted) = todo.evidence.as_ref().filter(|_| checks.is_empty()) {
                let mark = match resolution {
                    Some(Resolution::VerifiedDone) => Mark::Met,
                    Some(_) | None => Mark::Pending,
                };
                checks.push(Check {
                    text: quoted.clone(),
                    mark,
                });
            }
            let file = output.as_ref().filter(|url| url.scheme() == &Scheme::Local);
            place = file.and_then(|url| Address::new(Kind::File, url.path()).ok());
        }
    }
    if let Some(claim) = thread.work.claims.iter().find(|claim| claim.label == label) {
        let (check, seen) = claim_check(thread, claim);
        checks.push(check);
        evidence.extend(seen);
    }
    let severity = match checks.iter().any(|check| check.mark == Mark::Failing) {
        true => Severity::High,
        false => Severity::Low,
    };
    Some(Raw {
        id,
        title: label.to_owned(),
        severity,
        origin: Origin::Todo,
        place: place.or_else(|| home(thread))?,
        owner,
        age: None,
        evidence,
        checks,
        events,
    })
}

/// A done todo whose claim names no todo of the list: it still says something was done.
fn claim_raw(thread: &Thread, claim: &Claim) -> Option<Raw> {
    let (check, seen) = claim_check(thread, claim);
    Some(Raw {
        id: FindingId::new(format!("{}/claim/{}", thread.id, claim.label)),
        title: claim.label.clone(),
        severity: Severity::Low,
        origin: Origin::Claim,
        place: home(thread)?,
        owner: None,
        age: None,
        evidence: seen.into_iter().collect(),
        checks: vec![check],
        events: vec![Event::Assign, Event::Propose],
    })
}

/// A claim is met when the session holds the call it quotes; that call is its evidence.
fn claim_check(thread: &Thread, claim: &Claim) -> (Check, Option<Address>) {
    let seen = claim
        .observed
        .as_ref()
        .and_then(|call| Address::new(Kind::Step, format!("{}/{call}", thread.id)).ok());
    let mark = match seen {
        Some(_) => Mark::Met,
        None => Mark::Pending,
    };
    let text = "The session holds the call the evidence quotes".to_owned();
    (Check { text, mark }, seen)
}

fn goal_check(goal: &Goal) -> Option<Check> {
    let text = goal.check.clone()?;
    let mark = match (&goal.check_failure, &goal.status) {
        (Some(_), _) => Mark::Failing,
        (None, GoalStatus::Complete) => Mark::Met,
        (None, _) => Mark::Pending,
    };
    Some(Check { text, mark })
}

/// Whether a waiting ask of the thread would run `command`: the user's go-ahead is all it needs.
fn asked(model: &Model, thread: &Thread, command: &str) -> bool {
    model.wire.waiting().any(|ask| {
        ask.thread == thread.id
            && (ask.title.contains(command)
                || ask
                    .description
                    .as_deref()
                    .is_some_and(|text| text.contains(command)))
    })
}

/// The thread's folder as a place, else the thread itself.
fn home(thread: &Thread) -> Option<Address> {
    Address::new(Kind::Place, thread.cwd.as_str())
        .or_else(|_| Address::new(Kind::Thread, thread.id.as_str()))
        .ok()
}

fn made(model: &Model, shown: &ThreadId) -> Vec<(ThreadId, Raw)> {
    let made = model.findings.triage.made.iter();
    let here = made.filter(|made| made.thread.as_ref().is_none_or(|thread| thread == shown));
    here.map(|made| {
        let thread = made.thread.as_ref().unwrap_or(shown);
        let place = match made.evidence.kind() {
            Kind::Lines | Kind::File => Address::new(Kind::File, made.evidence.id()).ok(),
            _ => model.wire.threads.get(thread).and_then(home),
        };
        let raw = Raw {
            id: made.id.clone(),
            title: made.title.clone(),
            severity: Severity::Med,
            origin: Origin::You,
            place: place.unwrap_or_else(|| made.evidence.clone()),
            owner: None,
            age: Some(Millis(model.now.0.saturating_sub(made.at.0))),
            evidence: vec![made.evidence.clone()],
            checks: Vec::new(),
            events: Vec::new(),
        };
        (made.thread.clone().unwrap_or_else(|| shown.clone()), raw)
    })
    .collect()
}

/// One place and the open findings in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub place: Address,
    pub count: u32,
    /// It holds a High finding: its bar is drawn red.
    pub high: bool,
}

/// What the hub's header, groups and By place pane draw. Suggestions are counted apart and
/// never as open.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Counts {
    pub stages: BTreeMap<Stage, u32>,
    /// Open findings by severity.
    pub severities: BTreeMap<Severity, u32>,
    /// Open findings by place, most first.
    pub places: Vec<Place>,
    pub open: u32,
    pub suggestions: u32,
}

pub fn counts(findings: &[Finding]) -> Counts {
    let mut counts = Counts::default();
    let mut places: BTreeMap<&Address, (u32, bool)> = BTreeMap::new();
    for finding in findings {
        bump(counts.stages.entry(finding.stage).or_default());
        if finding.stage == Stage::New {
            bump(&mut counts.suggestions);
        }
        if finding.stage.open() {
            bump(&mut counts.open);
            bump(counts.severities.entry(finding.severity).or_default());
            let place = places.entry(&finding.place).or_default();
            bump(&mut place.0);
            place.1 |= finding.severity == Severity::High;
        }
    }
    counts.places = places
        .into_iter()
        .map(|(place, (count, high))| Place {
            place: place.clone(),
            count,
            high,
        })
        .collect();
    counts
        .places
        .sort_by_key(|place| std::cmp::Reverse(place.count));
    counts
}

fn bump(count: &mut u32) {
    *count = count.saturating_add(1);
}

/// The Ready? gate: what still blocks calling the work done. Suggestions never block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ready {
    /// Open findings of High severity.
    pub high: Vec<FindingId>,
    /// Fixes proposed whose checks have not all passed.
    pub unproven: Vec<FindingId>,
    /// Suggestions left to triage: shown, not blocking.
    pub untriaged: u32,
}

impl Ready {
    pub fn ready(&self) -> bool {
        self.high.is_empty() && self.unproven.is_empty()
    }

    pub fn blockers(&self) -> usize {
        self.high.len().saturating_add(self.unproven.len())
    }
}

pub fn ready(findings: &[Finding]) -> Ready {
    let mut gate = Ready::default();
    for finding in findings {
        match finding.stage {
            Stage::New => bump(&mut gate.untriaged),
            Stage::Done => {}
            Stage::Open | Stage::Fixing | Stage::Proving | Stage::Watching => {
                if finding.severity == Severity::High {
                    gate.high.push(finding.id.clone());
                }
                if finding.stage == Stage::Proving {
                    gate.unproven.push(finding.id.clone());
                }
            }
        }
    }
    gate
}

#[cfg(test)]
#[path = "view/tests.rs"]
mod tests;
