//! `yi doctor`: the invariants a session assumes, as one table, each row checked and the safe
//! ones repaired on `--fix` (plan 2026-09-05 §3.B).

use std::path::{Path, PathBuf};

use crate::{Args, effective_cwd, read_config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Fail,
    Fixed,
}

struct Finding {
    status: Status,
    detail: String,
}

fn ok(detail: impl Into<String>) -> Finding {
    Finding {
        status: Status::Ok,
        detail: detail.into(),
    }
}

fn fail(detail: impl Into<String>) -> Finding {
    Finding {
        status: Status::Fail,
        detail: detail.into(),
    }
}

struct Site {
    home: PathBuf,
    cwd: PathBuf,
    fix: bool,
}

type Check = fn(&Site) -> Finding;

/// The rows, in the order a reader wants them: what the process is, then what it owns.
const ROWS: [(&str, Check); 11] = [
    ("host", host_environment),
    ("home", home_absolute),
    ("config", config_parses),
    ("classifier", classifier_answers),
    ("catalog", catalog_age),
    ("python-runtime", python_runtime_present),
    ("kernel-toolchain", kernel_toolchain),
    ("kernel-boot", kernel_boot),
    ("daemon-socket", socket_alive_or_absent),
    ("daemon-ledger", ledger_roots_exist),
    ("lanes", lanes_consistent),
];

/// Never a failure: where yi runs decides what a contained command and a placement can reach.
fn host_environment(_site: &Site) -> Finding {
    ok(yi_runtime::host::facts().line())
}

fn home_absolute(site: &Site) -> Finding {
    if site.home.is_absolute() {
        ok(site.home.display().to_string())
    } else {
        fail(format!("{} is relative", site.home.display()))
    }
}

fn config_parses(site: &Site) -> Finding {
    if !site.home.join(".yi/config.json").exists() {
        return ok("none yet; `yi setup` writes one");
    }
    match read_config(&site.home) {
        Ok((_, migrations)) => ok(migrations
            .iter()
            .fold(String::from("~/.yi/config.json"), |row, migration| {
                format!("{row}; {migration}")
            })),
        Err(error) => fail(error),
    }
}

fn classifier_answers(site: &Site) -> Finding {
    let Ok((config, _)) = read_config(&site.home) else {
        return ok("see config");
    };
    let Some(model) = config.models.and_then(|roles| roles.classifier) else {
        return ok("off; `yi setup` offers it");
    };
    let url = config
        .classifier
        .and_then(|block| block.url)
        .unwrap_or_else(|| yi_runtime::classifier::DEFAULT_URL.to_owned());
    match yi_runtime::classifier::probe(&url, &model) {
        Ok(()) => ok(format!("{model} answers at {url}")),
        Err(error) => fail(format!(
            "no answer at {url} ({error}); start laya-serve or run `yi setup`"
        )),
    }
}

fn catalog_age(site: &Site) -> Finding {
    let hours = read_config(&site.home)
        .ok()
        .and_then(|(config, _)| config.catalog.and_then(|catalog| catalog.refresh_hours))
        .unwrap_or(yi_runtime::DEFAULT_REFRESH_HOURS);
    let dir = site.home.join(".yi/catalog");
    let now = crate::catalog::clock();
    let stale: Vec<String> = yi_runtime::CATALOG_PROVIDERS
        .iter()
        .filter(|provider| {
            yi_runtime::catalog_age(&dir, provider, now)
                .is_some_and(|age| age.as_secs() >= hours.saturating_mul(3600))
        })
        .map(|provider| (*provider).to_owned())
        .collect();
    if stale.is_empty() {
        ok("bundled floor, caches younger than the refresh age")
    } else {
        fail(format!(
            "{} older than {hours}h; `yi catalog refresh`",
            stale.join(", ")
        ))
    }
}

/// Invariant: the root the kernel installs from holds a tree (#278: a binary away from its
/// checkout resolved to the build machine's path). `--fix` unpacks the embed under ~/.yi.
fn python_runtime_present(site: &Site) -> Finding {
    let root = yi_runtime::python_root();
    if root.join("yi_runtime").is_dir() || root.join("skills").is_dir() {
        return ok(root.display().to_string());
    }
    if site.fix {
        return match yi_runtime::unpack_embedded_python(&site.home) {
            Ok(root) => Finding {
                status: Status::Fixed,
                detail: format!("embedded runtime unpacked to {}", root.display()),
            },
            Err(error) => fail(error),
        };
    }
    fail(format!(
        "{} holds neither yi_runtime nor skills; `yi doctor --fix` unpacks the embed",
        root.display()
    ))
}

fn kernel_toolchain(site: &Site) -> Finding {
    match yi_runtime::doctor_toolchain(&site.home) {
        Ok(detail) => ok(detail),
        Err(error) => fail(error),
    }
}

/// Builds under `--fix` (the trial adapter's install step), then boots a real kernel and times it.
fn kernel_boot(site: &Site) -> Finding {
    match yi_runtime::doctor_boot(&site.home, site.fix) {
        Ok((true, detail)) => Finding {
            status: Status::Fixed,
            detail,
        },
        Ok((false, detail)) => ok(detail),
        Err(error) => fail(error),
    }
}

fn socket_path(site: &Site) -> PathBuf {
    site.home.join(".yi/daemon.sock")
}

fn socket_alive_or_absent(site: &Site) -> Finding {
    let path = socket_path(site);
    if !path.exists() {
        return ok("no daemon");
    }
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return ok("daemon answers");
    }
    if site.fix {
        return match std::fs::remove_file(&path) {
            Ok(()) => Finding {
                status: Status::Fixed,
                detail: "dead socket file removed".to_owned(),
            },
            Err(error) => fail(format!("dead socket file, not removable: {error}")),
        };
    }
    fail("dead socket file; `yi doctor --fix` removes it")
}

/// Invariant: a ledger row names a cwd a session can be resumed in; the daemon owns the file
/// (D118), so a gone root is reported here and pruned there.
fn ledger_roots_exist(site: &Site) -> Finding {
    let path = socket_path(site).with_extension("ledger.json");
    let Ok(raw) = std::fs::read(&path) else {
        return ok("no ledger");
    };
    let Ok(ledger) = serde_json::from_slice::<yi_types::acp::DaemonLedger>(&raw) else {
        return fail(format!("{} does not parse", path.display()));
    };
    let gone: Vec<String> = ledger
        .sessions
        .iter()
        .filter(|(_, entry)| !Path::new(&entry.cwd).is_dir())
        .map(|(id, entry)| format!("{id} ({})", entry.cwd))
        .collect();
    if gone.is_empty() {
        ok(format!("{} rows, every root exists", ledger.sessions.len()))
    } else {
        fail(format!(
            "{} row(s) whose root is gone: {}",
            gone.len(),
            gone.join(", ")
        ))
    }
}

fn lanes_consistent(site: &Site) -> Finding {
    use yi_runtime::lane::{LaneError, Pool, SlotView, land::lane_line};
    let slots = crate::lanes::configured_lanes()
        .slots
        .unwrap_or(yi_runtime::lane::DEFAULT_SLOTS);
    let pool = match Pool::open(&site.home, &site.cwd, slots) {
        Ok(pool) => pool,
        Err(LaneError::NotARepo(_)) => return ok("not a repository"),
        Err(error) => return fail(error.to_string()),
    };
    let views = match pool.list() {
        Ok(views) => views,
        Err(error) => return fail(error.to_string()),
    };
    let mut fixed = Vec::new();
    let mut left = Vec::new();
    for view in &views {
        let SlotView::Orphan { slot, .. } = view else {
            continue;
        };
        if site.fix {
            match pool.reap(*slot) {
                Ok(note) => fixed.push(note),
                Err(error) => left.push(format!("{}: {error}", lane_line(view, None))),
            }
        } else {
            left.push(format!("{}; `yi lanes reap {slot}`", lane_line(view, None)));
        }
    }
    if !left.is_empty() {
        return fail(left.join("; "));
    }
    if !fixed.is_empty() {
        return Finding {
            status: Status::Fixed,
            detail: fixed.join("; "),
        };
    }
    ok(format!("{} slot(s) consistent", views.len()))
}

/// Runs before the config loads, so a config that will not parse is a row, not a death.
pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() != Some("doctor") {
        return;
    }
    match crate::parse_args() {
        Ok(args) => std::process::exit(run(&args)),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    }
}

pub(crate) fn run(args: &Args) -> i32 {
    let site = Site {
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        cwd: effective_cwd(args),
        fix: args.fix,
    };
    let findings: Vec<(&str, Finding)> = ROWS
        .iter()
        .map(|(name, check)| (*name, check(&site)))
        .collect();
    if args.json {
        let rows: Vec<serde_json::Value> = findings
            .iter()
            .map(|(name, finding)| {
                serde_json::json!({
                    "name": name,
                    "status": match finding.status { Status::Ok => "ok", Status::Fail => "fail", Status::Fixed => "fixed" },
                    "detail": finding.detail,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(rows));
    } else {
        for (name, finding) in &findings {
            let mark = match finding.status {
                Status::Ok => "ok   ",
                Status::Fail => "FAIL ",
                Status::Fixed => "fixed",
            };
            println!("{mark} {name:<14} {}", finding.detail);
        }
    }
    i32::from(
        findings
            .iter()
            .any(|(_, finding)| finding.status == Status::Fail),
    )
}
