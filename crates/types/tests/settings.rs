use serde_json::{Value, json};
use yi_types::config::{
    BashConfig, CatalogConfig, ClassifierConfig, CompactionConfig, ConsoleConfig, EditConfig,
    KernelConfig, MailConfig, McpConfig, ModelRoles, NodeConfig, PermissionsConfig, PlanConfig,
    PlansConfig, RlmConfig, SkillsConfig, SpendConfig, TelemetryConfig, TuiConfig, UserConfig,
};
use yi_types::lane::LanesConfig;
use yi_types::settings::{SettingKind, SettingRow, refusal, rows};

/// A pattern with no `..`: a field added to the struct stops this compiling until it is named.
macro_rules! fields {
    ($ty:ident { $($field:ident),* $(,)? }) => {{
        let $ty { $($field: _),* } = $ty::default();
        vec![$(stringify!($field)),*]
    }};
}

fn camel(snake: &str) -> String {
    let mut parts = snake.split('_');
    let head = parts.next().unwrap_or_default().to_owned();
    parts.fold(head, |mut out, part| {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
        out
    })
}

fn leaves() -> Vec<String> {
    let top = fields!(UserConfig {
        model,
        thinking,
        models,
        bash,
        plan,
        plans,
        mcp,
        kernel,
        edit,
        keys,
        console,
        tui,
        lanes,
        catalog,
        telemetry,
        routing,
        rlm,
        spend,
        compaction,
        node,
        skills,
        classifier,
        permissions,
        mail,
        open,
    });
    let nested = [
        (
            "models",
            fields!(ModelRoles {
                primary,
                summarizer,
                advisor,
                auto_review,
                classifier,
                selector
            }),
        ),
        ("bash", fields!(BashConfig { auto_background_ms })),
        (
            "plan",
            fields!(PlanConfig {
                stale_reminder_turns
            }),
        ),
        ("plans", fields!(PlansConfig { dir })),
        (
            "mcp",
            fields!(McpConfig {
                enabled,
                token_store
            }),
        ),
        ("kernel", fields!(KernelConfig { prewarm })),
        ("edit", fields!(EditConfig { freeform_grammar })),
        ("console", fields!(ConsoleConfig { auto_side })),
        ("tui", fields!(TuiConfig { pace })),
        (
            "lanes",
            fields!(LanesConfig {
                enabled,
                slots,
                land
            }),
        ),
        (
            "catalog",
            fields!(CatalogConfig {
                enabled,
                refresh_hours
            }),
        ),
        ("telemetry", fields!(TelemetryConfig { enabled })),
        ("rlm", fields!(RlmConfig { max_depth })),
        (
            "spend",
            fields!(SpendConfig {
                alert_tokens,
                alert_usd,
                day_alert_usd
            }),
        ),
        ("compaction", fields!(CompactionConfig { at })),
        ("node", fields!(NodeConfig { slots, isolation })),
        ("skills", fields!(SkillsConfig { global })),
        (
            "classifier",
            fields!(ClassifierConfig {
                url,
                timeout_ms,
                threshold,
                approve,
                approval,
                allow_at,
                allow_destructive_at,
                ask_at,
                ask_timeout_secs,
            }),
        ),
        ("permissions", fields!(PermissionsConfig { mode })),
        ("mail", fields!(MailConfig { hop_limit })),
    ];
    top.into_iter()
        .flat_map(
            |field| match nested.iter().find(|(section, _)| *section == field) {
                Some((section, inner)) => inner
                    .iter()
                    .map(|leaf| format!("{section}.{}", camel(leaf)))
                    .collect(),
                None => vec![camel(field)],
            },
        )
        .collect()
}

fn sample(row: &SettingRow) -> Value {
    match &row.kind {
        SettingKind::Toggle => json!(true),
        SettingKind::Choice { options } => json!(options.first()),
        SettingKind::Number { min, .. } => min.clone().map_or(json!(1), Value::Number),
        SettingKind::Text => json!("english"),
        SettingKind::Model => json!("openrouter/z-ai/glm-5.3-flash"),
        SettingKind::List => json!(["one"]),
        SettingKind::Json if row.id == "open" => {
            json!([{"name": "zed", "command": ["zed", "{path}"]}])
        }
        SettingKind::Json => json!({}),
    }
}

fn set(config: &mut Value, id: &str, value: Value) {
    let mut at = config;
    let mut keys = id.split('.').peekable();
    while let Some(key) = keys.next() {
        if keys.peek().is_none() {
            at[key] = value;
            return;
        }
        at = &mut at[key];
    }
}

/// The page shows everything in the config: a leaf with no row is a setting nobody can find.
#[test]
fn every_config_leaf_has_exactly_one_row() {
    let mut ids: Vec<String> = rows().into_iter().map(|row| row.id).collect();
    let mut want = leaves();
    ids.sort();
    want.sort();
    assert_eq!(ids, want);
}

/// A row is only as good as the config it writes: every row's own sample, saved at its path,
/// must be a config the strict parse loads.
#[test]
fn every_row_saves_a_value_the_config_loads() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = json!({});
    for row in rows() {
        let value = sample(&row);
        assert_eq!(
            refusal(&row, &value),
            None,
            "{} refused its own sample",
            row.id
        );
        set(&mut config, &row.id, value);
    }
    yi_types::config::parse(&config.to_string())?;
    Ok(())
}

#[test]
fn a_value_past_a_bound_or_of_the_wrong_type_is_refused_with_the_rule() -> Result<(), String> {
    let all = rows();
    let find = |id: &str| {
        all.iter()
            .find(|row| row.id == id)
            .ok_or(format!("no {id} row"))
    };
    let depth = find("rlm.maxDepth")?;
    assert_eq!(refusal(depth, &json!(3)), None);
    assert_eq!(
        refusal(depth, &json!(4)).as_deref(),
        Some("rlm.maxDepth takes at most 3")
    );
    assert_eq!(
        refusal(depth, &json!(1.5)).as_deref(),
        Some("rlm.maxDepth takes a whole number")
    );
    let threshold = find("classifier.threshold")?;
    assert_eq!(refusal(threshold, &json!(0.85)), None);
    assert_eq!(
        refusal(threshold, &json!(-0.1)).as_deref(),
        Some("classifier.threshold takes at least 0")
    );
    let mode = find("permissions.mode")?;
    assert_eq!(
        refusal(mode, &json!("never")).as_deref(),
        Some("permissions.mode takes one of auto, ask, yolo")
    );
    assert_eq!(refusal(mode, &Value::Null), None, "unset is always allowed");
    Ok(())
}
