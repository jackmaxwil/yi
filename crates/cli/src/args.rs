use crate::user_config::{config, configured_mode, configured_model};
use crate::{debug, stats, trace_sink};
use yi_types::model::{Effort, UnknownEffort};

#[derive(Clone)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) help: bool,
    pub(crate) flags: Vec<String>,
    pub(crate) model: String,
    pub(crate) system: String,
    pub(crate) thinking: Option<Effort>,
    /// `--model` was named on the command line, so it outranks a resumed one.
    pub(crate) model_pinned: bool,
    pub(crate) thinking_pinned: bool,
    pub(crate) json: bool,
    pub(crate) mode: yi_runtime::PermissionMode,
    pub(crate) mode_pinned: bool,
    pub(crate) session_dir: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) here: bool,
    pub(crate) fix: bool,
    pub(crate) socket: Option<String>,
    pub(crate) headless: bool,
    pub(crate) solo: bool,
    pub(crate) keys: Option<String>,
    pub(crate) control: Option<String>,
    pub(crate) frames: Option<String>,
    pub(crate) faux: Option<String>,
    pub(crate) record: Option<String>,
    pub(crate) snap: Option<String>,
    /// `--size COLSxROWS`: the headless screen, 80x24 when absent.
    pub(crate) size: Option<(u16, u16)>,
    /// An eval harness launched this run: `YI_LEVERS` is read, and nothing else (D220).
    pub(crate) eval: bool,
    pub(crate) deadline: Option<u64>,
    pub(crate) resume: Resume,
    pub(crate) schema: Option<String>,
    pub(crate) stats: stats::Query,
    pub(crate) prompt: String,
}

/// Which session file `yi ask` writes to (§17.1 `--continue` / `--session`).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Resume {
    Fresh,
    Leaf,
    Named(String),
}

pub(crate) fn parse_args() -> Result<Args, lexopt::Error> {
    parse_from(std::env::args_os())
}

pub(super) fn parse_from(
    argv: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
) -> Result<Args, lexopt::Error> {
    use lexopt::prelude::*;
    let mut command = String::new();
    let mut help = false;
    let mut given = Vec::new();
    let mut model = None;
    let mut system = String::new();
    let mut thinking = None;
    let mut json = false;
    let mut mode = None;
    let mut session_dir = None;
    let mut cwd = None;
    let mut here = false;
    let mut fix = false;
    let mut socket = None;
    let mut headless = false;
    let mut lanes = false;
    let mut solo = false;
    let mut keys = None;
    let mut control = None;
    let mut frames = None;
    let mut faux = None;
    let mut record = None;
    let mut snap = None;
    let mut size = None;
    let mut eval = false;
    let mut deadline = None;
    let mut continue_leaf = false;
    let mut session = None;
    let mut schema = None;
    let mut stats_flags = stats::Flags::default();
    let mut prompt_parts: Vec<String> = Vec::new();
    let mut parser = lexopt::Parser::from_iter(argv);
    while let Some(argument) = parser.next()? {
        if let Long(name) = argument {
            if crate::commands::find_flag(name).is_none() {
                return Err(argument.unexpected());
            }
            given.push(name.to_owned());
        }
        match argument {
            Long("help") | Short('h') => help = true,
            Long("version") => command = "version".to_owned(),
            Long("model") => model = Some(parser.value()?.string()?),
            Long("system") => system = parser.value()?.string()?,
            Long("thinking") => thinking = Some(parse_effort(&parser.value()?.string()?)?),
            Long("json") => json = true,
            Long("yolo") => mode = Some(yi_runtime::PermissionMode::Yolo),
            Long("auto") => mode = Some(yi_runtime::PermissionMode::Auto),
            Long("confirm") => mode = Some(yi_runtime::PermissionMode::Ask),
            Long("session-dir") => session_dir = Some(parser.value()?.string()?),
            Long("cwd") => cwd = Some(parser.value()?.string()?),
            Long("here") => here = true,
            Long("fix") => fix = true,
            Long("socket") => socket = Some(parser.value()?.string()?),
            Long("headless") => headless = true,
            Long("lanes") => lanes = true,
            Long("solo") => solo = true,
            Long("keys") => keys = Some(parser.value()?.string()?),
            Long("control") => control = Some(parser.value()?.string()?),
            Long("frames") => frames = Some(parser.value()?.string()?),
            Long("faux") => faux = Some(parser.value()?.string()?),
            Long("record") => record = Some(parser.value()?.string()?),
            Long("snap") => snap = Some(parser.value()?.string()?),
            Long("size") => size = Some(parse_size(&parser.value()?.string()?)?),
            Long("eval") => eval = true,
            Long("deadline") => deadline = Some(parser.value()?.parse()?),
            Long("continue") => continue_leaf = true,
            Long("session") => session = Some(parser.value()?.string()?),
            Long("schema") => schema = Some(parser.value()?.string()?),
            Long("since") if command == "stats" => {
                stats_flags.since = Some(parser.value()?.string()?)
            }
            Long("by") if command == "stats" => stats_flags.by = Some(parser.value()?.string()?),
            Long("top") if command == "stats" => stats_flags.top = Some(parser.value()?.string()?),
            Value(value) => {
                let value = value.string()?;
                if command.is_empty() {
                    command = value;
                } else {
                    prompt_parts.push(value);
                }
            }
            _ => return Err(argument.unexpected()),
        }
    }
    trace_sink::install();
    yi_types::trace::init(debug::process_label(&command));
    // A drive is a harness: it claims no lane unless the scenario is about lanes.
    // A cassette run is a harness too: a claim would fetch origin (#943).
    let here = here || ((headless || faux.is_some()) && !lanes);
    let args = Args {
        command,
        help,
        flags: given,
        model_pinned: model.is_some(),
        model: model.or_else(configured_model).unwrap_or_default(),
        system,
        thinking_pinned: thinking.is_some(),
        thinking: thinking.or(config().thinking),
        json,
        mode: mode.unwrap_or_else(configured_mode),
        mode_pinned: mode.is_some(),
        session_dir,
        cwd,
        here,
        fix,
        socket,
        headless,
        solo,
        keys,
        control,
        frames,
        faux,
        record,
        snap,
        size,
        eval,
        deadline,
        resume: match (session, continue_leaf) {
            (Some(id), _) => Resume::Named(id),
            (None, true) => Resume::Leaf,
            (None, false) => Resume::Fresh,
        },
        schema,
        stats: stats_flags.typed()?,
        prompt: prompt_parts.join(" "),
    };
    inert_flags(&args)?;
    Ok(args)
}

fn inert_flags(args: &Args) -> Result<(), lexopt::Error> {
    // Drive-only flags are silently inert outside the headless loop, which
    // reads downstream as a capture that produced nothing.
    if !args.headless
        && let Some(flag) = [
            ("--keys", args.keys.is_some()),
            ("--frames", args.frames.is_some()),
            ("--record", args.record.is_some()),
            ("--snap", args.snap.is_some()),
            ("--size", args.size.is_some()),
        ]
        .into_iter()
        .find_map(|(name, present)| present.then_some(name))
    {
        return Err(lexopt::Error::Custom(
            format!("{flag} needs --headless").into(),
        ));
    }
    // The console, headless too, is a client of a daemon that never sees the cassette, and
    // `yi serve` does not hand it to its workers (#943).
    let daemon = matches!(args.command.as_str(), "console" | "serve")
        || (args.command.is_empty() && !args.solo);
    if args.faux.is_some() && daemon {
        return Err(lexopt::Error::Custom(
            "--faux runs in-process only: use `yi tui`, `yi ask`, `yi rpc` or `yi --solo`".into(),
        ));
    }
    Ok(())
}

fn parse_effort(raw: &str) -> Result<Effort, lexopt::Error> {
    raw.parse()
        .map_err(|error: UnknownEffort| lexopt::Error::Custom(Box::new(error)))
}

/// `COLSxROWS` within 20..=1000 by 8..=500: the drive's own floor, and a screen that fits in memory.
fn parse_size(raw: &str) -> Result<(u16, u16), lexopt::Error> {
    raw.split_once('x')
        .and_then(|(cols, rows)| Some((cols.parse().ok()?, rows.parse().ok()?)))
        .filter(|(cols, rows)| (20..=1000).contains(cols) && (8..=500).contains(rows))
        .ok_or_else(|| {
            lexopt::Error::Custom(
                format!("--size wants COLSxROWS within 20..=1000 by 8..=500, not {raw}").into(),
            )
        })
}
