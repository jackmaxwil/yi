use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{Receiver, Sender};
use yi_ai::faux::{FAUX_API, FAUX_MODEL_ID, FAUX_PROVIDER, faux_assistant_message};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::{LoopConfig, LoopContext, REASONING_CHAR_CAP, run_loop};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::model::{LlmContext, Model};

#[path = "../../types/tests/support/model.rs"]
mod common;

/// A provider that streams a reasoning spiral past the cap and then never settles the turn:
/// no `Done`, and the channel stays open.
#[derive(Default)]
struct NeverSettles {
    held: Mutex<Vec<Sender<AssistantMessageEvent>>>,
}

impl yi_loop::run::StreamFn for NeverSettles {
    fn stream(
        &self,
        _model: &Model,
        _context: &LlmContext,
        _effort: yi_types::model::Effort,
        _signal: &InterruptSignal,
    ) -> Receiver<AssistantMessageEvent> {
        let partial = faux_assistant_message(Vec::new(), StopReason::Stop);
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        for event in [
            AssistantMessageEvent::Start { partial },
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "y".repeat(REASONING_CHAR_CAP),
            },
        ] {
            let _ = sender.try_send(event);
        }
        if let Ok(mut held) = self.held.lock() {
            held.push(sender);
        }
        receiver
    }
}

#[tokio::test(start_paused = true)]
async fn an_abort_during_the_cut_drain_returns_at_once() {
    let model = common::model(
        FAUX_MODEL_ID,
        FAUX_API,
        FAUX_PROVIDER,
        "http://localhost:0",
        None,
    );
    let config = LoopConfig::new(model);
    let signal = InterruptSignal::default();
    let cut = signal.cut_flag();
    let mut context = LoopContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let prompt = AgentMessage::user_input(UserContent::Text("go".to_owned()), 0);
    let stream = NeverSettles::default();
    let mut fired_at = None;
    let fire = async {
        while !cut.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        fired_at = Some(tokio::time::Instant::now());
        signal.fire();
    };
    let mut emit = |_| {};
    let run = run_loop(
        &mut context,
        vec![prompt],
        &config,
        &signal,
        &mut emit,
        &stream,
    );
    let (_collected, ()) = tokio::join!(run, fire);
    let waited = fired_at.map(|at| at.elapsed());
    assert!(
        waited.is_some_and(|waited| waited < std::time::Duration::from_millis(100)),
        "the loop returned {waited:?} after the abort"
    );
}

/// Each round snapshots the epoch, then a reset and a fire start together on two spinning
/// threads, the reset a little later each round; the fire came after the snapshot, so it must
/// survive whichever side wins.
#[test]
fn a_fire_racing_a_reset_is_never_lost() {
    const ROUNDS: usize = 10_000;
    let signal = Arc::new(InterruptSignal::default());
    let (round, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let epoch = Arc::new(AtomicU64::new(0));
    let worker = |act: fn(&InterruptSignal, u64, usize)| {
        let (signal, round, done, epoch) = (
            Arc::clone(&signal),
            Arc::clone(&round),
            Arc::clone(&done),
            Arc::clone(&epoch),
        );
        std::thread::spawn(move || {
            for at in 1..=ROUNDS {
                while round.load(Ordering::SeqCst) != at {
                    std::hint::spin_loop();
                }
                act(&signal, epoch.load(Ordering::SeqCst), at);
                done.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let firer = worker(|signal, _, _| signal.fire());
    let resetter = worker(|signal, epoch, at| {
        for _ in 0..at % 64 {
            std::hint::spin_loop();
        }
        signal.reset_if_epoch(epoch);
    });
    let mut lost = 0;
    for at in 1..=ROUNDS {
        epoch.store(signal.epoch(), Ordering::SeqCst);
        round.store(at, Ordering::SeqCst);
        while done.load(Ordering::SeqCst) != at * 2 {
            std::hint::spin_loop();
        }
        if !signal.is_fired() {
            lost += 1;
        }
        signal.reset_if_epoch(signal.epoch());
    }
    let _ = (firer.join(), resetter.join());
    assert_eq!(
        lost, 0,
        "{lost} of {ROUNDS} fires were lost to a racing reset"
    );
}
