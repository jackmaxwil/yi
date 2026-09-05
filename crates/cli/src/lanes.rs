//! `yi lanes`, and the root session's claim and release (D117).

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

/// D117: a root session claims a lane unless `--here`, `lanes.enabled: false`, or
/// the cwd is not a repository. A claim that fails fails the start: no trunk fallback.
pub(crate) fn claim_lane(
    args: &Args,
    home: &std::path::Path,
) -> Result<Option<yi_runtime::lane::Lane>, String> {
    let lanes = configured_lanes();
    if args.here || lanes.enabled == Some(false) {
        return Ok(None);
    }
    let slots = lanes.slots.unwrap_or(yi_runtime::lane::DEFAULT_SLOTS);
    let pool = match yi_runtime::lane::Pool::open(home, &effective_cwd(args), slots) {
        Ok(pool) => pool,
        Err(yi_runtime::lane::LaneError::NotARepo(_)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let session = resume_id(args).unwrap_or_else(|| format!("pid-{}", std::process::id()));
    yi_runtime::lane::land::claim_root(&pool, &session)
        .map(Some)
        .map_err(|error| error.to_string())
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
        _ => pool
            .list()
            .map(|views| yi_runtime::lane::land::format_lanes(&views))
            .map_err(|error| error.to_string()),
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
