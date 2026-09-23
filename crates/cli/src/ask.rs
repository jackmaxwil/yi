use std::time::{Duration, Instant};

use yi_runtime::Status;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, StopReason};

use super::{
    Args, Resume, attach_store, build_session, emit_structured, exit_refused, release_lane,
    render_text, tty,
};

const HOLD_POLL: Duration = Duration::from_millis(500);

pub(super) fn run(args: &Args) -> i32 {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    // Session wiring spawns runtime tasks (the H4 scheduler timer), so the
    // runtime context must exist before build_session.
    let (session, host) = {
        let _guard = runtime.enter();
        let asker: Option<yi_runtime::Asker> =
            interactive.then(|| std::sync::Arc::new(tty::tty_ask) as yi_runtime::Asker);
        match build_session(args, asker, None) {
            Ok(built) => built,
            Err(refused) => return exit_refused(refused),
        }
    };
    match attach_store(args, &session) {
        Ok(_id) => {}
        // A requested resume that cannot be honoured is an error; an
        // unavailable store for a fresh turn only costs the recording.
        Err(error) if args.resume == Resume::Fresh => {
            eprintln!("warning: session store unavailable: {error}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    }
    let json = args.json;
    let prompt = args.prompt.clone();
    let schema = match args.schema.as_deref().map(yi_runtime::schema::Schema::load) {
        Some(Ok(schema)) => Some(schema),
        Some(Err(error)) => {
            eprintln!("error: {error}");
            return 2;
        }
        None => None,
    };
    let lane = session.lane();
    let ends = args
        .deadline
        .and_then(|secs| Instant::now().checked_add(Duration::from_secs(secs)));
    let code = runtime.block_on(async move {
        if session
            .prompt_message(yi_runtime::session::user_input(&prompt))
            .is_err()
        {
            eprintln!("error: session busy");
            return 1;
        }
        stream(&session, &host, json, schema.as_ref(), ends).await
    });
    release_lane(lane.as_deref());
    code
}

fn holds(working: bool, ends: Option<Instant>) -> bool {
    working && ends.is_none_or(|at| Instant::now() < at)
}

fn patience(holding: bool, ends: Option<Instant>) -> Option<Duration> {
    let left = ends.map(|at| at.saturating_duration_since(Instant::now()));
    match (holding, left) {
        (true, left) => Some(left.map_or(HOLD_POLL, |left| left.min(HOLD_POLL))),
        (false, left) => left,
    }
}

async fn stream(
    session: &yi_runtime::AgentSession,
    host: &yi_runtime::SubagentHost,
    json: bool,
    schema: Option<&yi_runtime::schema::Schema>,
    ends: Option<Instant>,
) -> i32 {
    let held = || host.holds_owner();
    let working = || host.holds_owner() || session.status() != Status::Idle;
    follow(session.subscribe(), [&held, &working], json, schema, ends).await
}

/// Incident: a plan child's finish wakes the owner, so it waits, but only on what wakes it (#489).
async fn follow(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    [held, working]: [&dyn Fn() -> bool; 2],
    json: bool,
    schema: Option<&yi_runtime::schema::Schema>,
    ends: Option<Instant>,
) -> i32 {
    let (mut answer, mut exit) = (String::new(), 0);
    let (mut holding, mut ended) = (false, false);
    loop {
        let next = match patience(holding, ends) {
            None => events.recv().await,
            Some(wait) if wait.is_zero() => {
                ended = true;
                break;
            }
            Some(wait) => match tokio::time::timeout(wait, events.recv()).await {
                Ok(next) => next,
                Err(_) if holds(holding && working(), ends) => continue,
                Err(_) => {
                    ended = true;
                    break;
                }
            },
        };
        let event = match next {
            Ok(event) => event,
            // A slow reader is not the end of the run: breaking here exited 0 mid-turn.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                eprintln!("warning: {missed} events dropped behind a slow reader");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if json && let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
        if let Some(chunk) = render_text(&event) {
            if schema.is_some() {
                answer.push_str(&chunk);
            } else if !json {
                print!("{chunk}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
        match &event {
            AgentEvent::MessageEnd {
                message:
                    AgentMessage::Assistant {
                        stop_reason: StopReason::Error,
                        error_message,
                        ..
                    },
            } if !json => {
                eprintln!(
                    "error: {}",
                    error_message.as_deref().unwrap_or("provider error")
                );
                exit = 1;
            }
            AgentEvent::AgentStart if holding => {
                holding = false;
                answer.clear();
            }
            AgentEvent::AgentEnd { .. } if held() => holding = true,
            AgentEvent::AgentEnd { .. } => {
                ended = true;
                break;
            }
            _ => {}
        }
    }
    if ended && let Some(schema) = schema {
        exit = emit_structured(schema, &answer, json);
    } else if ended && !json {
        println!();
    }
    exit
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use yi_types::event::AgentEvent;

    /// Dies with the deadline unread: a finish that never settles holds `yi ask` open forever.
    #[test]
    fn a_hold_ends_at_the_deadline() {
        assert!(super::holds(true, None));
        assert!(!super::holds(true, Some(Instant::now())));
        assert!(!super::holds(false, None));
    }

    /// Dies with the deadline read only while holding: a Steer's owner turn ran unbounded.
    #[test]
    fn a_steered_turn_past_the_deadline_ends_the_run() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (events, receiver) = tokio::sync::broadcast::channel(8);
        events.send(AgentEvent::AgentEnd {
            messages: Vec::new(),
        })?;
        events.send(AgentEvent::AgentStart)?;
        let always = || true;
        let ends = Instant::now().checked_add(Duration::from_millis(50));
        let run = super::follow(receiver, [&always, &always], true, None, ends);
        let ran =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), run).await });
        assert_eq!(ran.ok(), Some(0), "the steered turn outlived the deadline");
        drop(events);
        Ok(())
    }
}
