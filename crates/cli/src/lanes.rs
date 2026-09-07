//! `yi lanes`, and the root session's claim and release (D119).

use crate::{Args, Resume, config, default_session_dir, effective_cwd, sessions};

pub(crate) fn configured_lanes() -> yi_types::lane::LanesConfig {
    config().lanes.clone().unwrap_or_default()
}

fn resume_id(args: &Args) -> Option<String> {
    match &args.resume {
        Resume::Fresh => None,
        Resume::Named(id) => Some(id.clone()),
        Resume::Leaf => {
            let mut repo = yi_runtime::session_store::JsonlRepo::new(
                default_session_dir(args),
                effective_cwd(args).display().to_string(),
            );
            sessions::latest_id(&mut repo)
        }
    }
}

pub(crate) type Claimed = (
    Option<yi_runtime::lane::Lane>,
    Option<yi_runtime::lane::Pool>,
);

/// D119: a root session claims a lane unless `--here`, `lanes.enabled: false` or no
/// repository; a full pool at an interactive start asks, outside the pool lock (D142).
pub(crate) fn claim_lane(
    args: &Args,
    home: &std::path::Path,
    session: Option<&str>,
) -> Result<Claimed, String> {
    use yi_runtime::lane::{LaneError, Pool, land::claim_root};
    let lanes = configured_lanes();
    let slots = lanes.slots.unwrap_or(yi_runtime::lane::DEFAULT_SLOTS);
    let pool = match Pool::open(home, &effective_cwd(args), slots) {
        Ok(pool) => pool,
        Err(LaneError::NotARepo(_)) => return Ok((None, None)),
        Err(error) => return Err(error.to_string()),
    };
    if args.here || lanes.enabled == Some(false) {
        return Ok((None, Some(pool)));
    }
    let session = session
        .map(str::to_owned)
        .or_else(|| resume_id(args))
        .unwrap_or_else(|| format!("pid-{}", std::process::id()));
    let lane = match claim_root(&pool, &session) {
        Ok(lane) => lane,
        Err(LaneError::PoolFull { .. }) if interactive() && !args.headless => {
            offer_left_slot(&pool)?;
            claim_root(&pool, &session).map_err(|error| error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    Ok((Some(lane), Some(pool)))
}

fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Asked with no lock held, applied only if the slot still names that session; an
/// unread tree is never offered as lossless, and only a lossless slot is the default.
fn offer_left_slot(pool: &yi_runtime::lane::Pool) -> Result<(), String> {
    use yi_runtime::lane::{Holder, SlotIndex, SlotView, land::lane_line};
    let views = pool.list().map_err(|error| error.to_string())?;
    let left: Vec<(&SlotView, &Holder)> = views
        .iter()
        .filter_map(|view| match view {
            SlotView::Orphan { holder, .. } => Some((view, holder)),
            SlotView::Idle { .. } | SlotView::Held { .. } => None,
        })
        .collect();
    if left.is_empty() {
        return Err("no free lane and nothing left to take; `yi lanes` lists the pool".to_owned());
    }
    eprintln!("no free lane. left behind:");
    for (view, _) in &left {
        eprintln!("  {}", lane_line(view, None));
    }
    let safe = left
        .iter()
        .find(|(_, holder)| holder.tree.is_empty())
        .map(|(view, _)| view.slot());
    let choices: Vec<String> = left
        .iter()
        .map(|(view, _)| view.slot().to_string())
        .collect();
    match safe {
        Some(slot) => eprint!("take {slot}? [Y/n/{}] ", choices.join("/")),
        None => eprint!("take which? [{}/n] ", choices.join("/")),
    }
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("prompt: {error}"))?;
    let chosen = match answer.trim() {
        "" | "y" | "Y" => safe,
        text => text.parse::<SlotIndex>().ok(),
    };
    let Some(slot) = chosen else {
        return Err("no lane taken; `yi lanes` lists the pool".to_owned());
    };
    let (_, holder) = left
        .iter()
        .find(|(view, _)| view.slot() == slot)
        .ok_or_else(|| format!("lane {slot} was not offered"))?;
    let note = pool
        .reap_left_by(slot, &holder.session)
        .map_err(|error| error.to_string())?;
    eprintln!("{note}");
    Ok(())
}

pub(crate) fn release_lane(lane: Option<&yi_runtime::lane::land::LaneHandle>) {
    if let Some(lane) = lane
        && let Err(error) = lane.release()
    {
        eprintln!("warning: lane: {error}");
    }
}

pub(crate) fn run_lanes(args: &Args) -> i32 {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let slots = configured_lanes()
        .slots
        .unwrap_or(yi_runtime::lane::DEFAULT_SLOTS);
    let pool = match yi_runtime::lane::Pool::open(&home, &effective_cwd(args), slots) {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let result = match args.prompt.split_once(' ') {
        Some(("reap", slot)) => slot
            .parse::<yi_runtime::lane::SlotIndex>()
            .map_err(|error| format!("reap: {error}"))
            .and_then(|slot| pool.reap(slot).map_err(|error| error.to_string())),
        _ => {
            let ledger = yi_acp::daemon::read_ledger(&crate::shells::daemon_socket(args));
            pool.list()
                .map(|views| yi_runtime::lane::land::format_lanes(&views, ledger.as_ref()))
                .map_err(|error| error.to_string())
        }
    };
    match result {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}
