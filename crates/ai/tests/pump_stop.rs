use std::error::Error;

use yi_ai::request::pump_sse;
use yi_loop::interrupt::InterruptSignal;

/// Kimi's live chat stream for a bash call, 91 events, recorded on 2026-10-24.
const RECORDED: &str = include_str!("fixtures/kimi-chat-2026-10-24-bash.sse");

#[test]
fn an_abort_stops_the_provider_pump_at_the_next_event() -> Result<(), Box<dyn Error>> {
    let signal = InterruptSignal::default();
    let response = ureq::Response::new(200, "OK", RECORDED)?;
    let mut read = 0;
    pump_sse(response, Some(&signal.cut_flag()), |_| {
        read += 1;
        if read == 3 {
            signal.fire();
        }
        Ok(true)
    })?;
    assert_eq!(
        read, 3,
        "the pump read {read} events after an abort at the third"
    );
    Ok(())
}
