//! `yi models recommend [--role R]` and `yi models refresh`: suggestions only, never applied.

use yi_runtime::recommend::{self, Mix, ROLES};
use yi_runtime::rollup::{DAY_MS, day_label, now_ms, scan};

use crate::{Args, default_session_dir, proxy_from_env, yi_ai_key};

const LEDGER_DAYS: u64 = 7;

fn ledger_mix(args: &Args) -> Option<Mix> {
    let since = now_ms().saturating_sub(LEDGER_DAYS.saturating_mul(DAY_MS));
    let sums = scan(&default_session_dir(args), since)
        .requests
        .iter()
        .filter(|request| !request.child)
        .fold([0i64; 4], |sums, request| {
            let usage = &request.usage;
            [
                sums[0].saturating_add(usage.input),
                sums[1].saturating_add(usage.cache_read),
                sums[2].saturating_add(usage.cache_write),
                sums[3].saturating_add(usage.output),
            ]
        });
    Mix::of(sums[0], sums[1], sums[2], sums[3])
}

fn refresh(dir: &std::path::Path) -> i32 {
    let Some(key) = yi_ai_key("openrouter") else {
        eprintln!("error: {}", yi_runtime::auth::missing_message("openrouter"));
        return 1;
    };
    let now = now_ms();
    let day = day_label(
        &yi_runtime::schedule::Zone::local(),
        now.saturating_sub(DAY_MS),
    );
    let fetched = proxy_from_env().and_then(|proxy| {
        recommend::refresh(
            dir,
            recommend::BASE,
            key.expose(),
            &day,
            now,
            proxy.as_ref(),
        )
    });
    match fetched {
        Ok(count) => {
            println!(
                "{count} benchmark rows written to {}",
                dir.join(recommend::SNAPSHOT).display()
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn show(args: &Args, dir: &std::path::Path) -> i32 {
    let roles: Vec<_> = match args.role.as_deref() {
        None => ROLES.iter().collect(),
        Some(name) => match recommend::role(name) {
            Some(role) => vec![role],
            None => {
                let names: Vec<_> = ROLES.iter().map(|role| role.name).collect();
                eprintln!("error: no role {name:?}; one of {}", names.join(", "));
                return 2;
            }
        },
    };
    let snapshot = match recommend::load(dir) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let age_h = now_ms().saturating_sub(recommend::fetched_ms(&snapshot)) / 3_600_000;
    let routable = yi_ai_key("openrouter").is_some();
    let models = yi_runtime::available_models();
    let (candidates, drops) = recommend::candidates(&snapshot, &models, &|_| routable);
    println!("{}", recommend::citation(&snapshot));
    println!("snapshot fetched {age_h}h ago; suggestions only, nothing is applied");
    println!(
        "[{} of {} openrouter catalog models scored: {} without an intelligence index, {} without a credential, {} without tool calling]",
        drops.scored, drops.catalog, drops.no_benchmark, drops.no_credential, drops.no_tools
    );
    let ledger = ledger_mix(args);
    let measured = ledger.map_or("the default 97.8% cache-hit mix", |_| "your last 7 days");
    for role in roles {
        let mix = role.mix.or(ledger).unwrap_or(Mix::DEFAULT);
        let ranked = recommend::recommend(role, &mix, &candidates);
        let source = if role.mix.is_some() {
            "this role's own mix"
        } else {
            measured
        };
        println!(
            "\n{}: intelligence^{} per blended $/Mtok, floor {}, context >= {}; mix {} ({source})",
            role.name,
            role.exponent,
            role.floor,
            role.min_context,
            mix.line()
        );
        if ranked.below_floor > 0 || ranked.short_context > 0 {
            println!(
                "  [{} under the intelligence floor and {} under the context floor left out]",
                ranked.below_floor, ranked.short_context
            );
        }
        if ranked.picks.is_empty() {
            println!("  no candidate clears this role's floors");
        }
        for pick in &ranked.picks {
            println!(
                "  {:<7}{:<44}{}",
                pick.tag,
                pick.id,
                pick.arithmetic(role.exponent)
            );
        }
    }
    0
}

pub(crate) fn run(args: &Args) -> i32 {
    let dir = crate::user_config::home().join(".yi/catalog");
    match args.prompt.as_str() {
        "recommend" | "" => show(args, &dir),
        "refresh" => refresh(&dir),
        other => {
            eprintln!("usage: yi models [recommend [--role R] | refresh] (not {other:?})");
            2
        }
    }
}
