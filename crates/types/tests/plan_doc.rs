use serde_json::Map;
use yi_types::plan::doc::{
    AgentId, BlockedOn, Check, DocError, GoalText, Isolation, Plan, PlanId, PlanIssue, PlanState,
    PlanTier, ProbeCommand, RetryCount, SPAWN_CAP, Todo, TodoAddr, TodoLabel, TodoState,
    terminal_durability,
};
use yi_types::url::{Durability, Url};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn slug_matches_the_golden_fixtures() -> TestResult {
    let cases = [
        (
            "Reach the deepest level of ls20 that the rules allow",
            "reach-the-deepest-level-of-ls20-that",
        ),
        (
            "Ship logrotate-lite with a packaged tarball",
            "ship-logrotate-lite-with-a-packaged",
        ),
        (
            "Implement rotation with size and age triggers",
            "implement-rotation-with-size-and-age",
        ),
    ];
    for (goal, expected) in cases {
        assert_eq!(PlanId::slug(goal)?.as_str(), expected);
    }
    let parent = PlanId::slug("Ship logrotate-lite with a packaged tarball")?;
    let label = TodoLabel::new("Implement rotation with size and age triggers")?;
    assert_eq!(
        parent.child(&label)?.as_str(),
        "ship-logrotate-lite-with-a-packaged.implement-rotation-with-size-and-age"
    );
    assert!(parent.child(&label)?.child(&label).is_err());
    Ok(())
}

fn plan_with(todos: Vec<Todo>) -> Result<Plan, DocError> {
    Ok(Plan::opening(
        PlanId::new("p")?,
        GoalText::new("g")?,
        PlanTier::Root,
        todos,
    ))
}

fn todo(label: &str, after: &[&str], state: TodoState) -> Result<Todo, DocError> {
    Ok(Todo {
        label: TodoLabel::new(label)?,
        after: after
            .iter()
            .map(|name| TodoLabel::new(*name))
            .collect::<Result<_, _>>()?,
        state,
        delegation: None,
        subplan: None,
        retries: RetryCount::default(),
        children: Vec::new(),
        note: None,
        attempt: yi_types::plan::doc::AttemptId::FIRST,
        refusals: 0,
        extra: Map::new(),
    })
}

#[test]
fn ready_finished_and_validate() -> TestResult {
    let plan = plan_with(vec![
        todo("a", &[], TodoState::Done { output: None })?,
        todo("b", &["a"], TodoState::Pending)?,
        todo("c", &["b"], TodoState::Pending)?,
    ])?;
    let ready: Vec<&str> = plan
        .ready()
        .iter()
        .map(|todo| todo.label.as_str())
        .collect();
    assert_eq!(ready, ["b"]);
    assert!(!plan.finished());
    assert!(plan.validate().is_empty());

    let done = plan_with(vec![
        todo("a", &[], TodoState::Done { output: None })?,
        todo("b", &[], TodoState::Abandoned)?,
        todo(
            "c",
            &[],
            TodoState::Failed {
                cause: "x".to_owned(),
                last: None,
            },
        )?,
    ])?;
    assert!(done.finished());

    let cyclic = plan_with(vec![
        todo("a", &["b"], TodoState::Pending)?,
        todo("b", &["a"], TodoState::Pending)?,
        todo("b", &["missing"], TodoState::Pending)?,
    ])?;
    let issues = cyclic.validate();
    assert!(
        issues
            .iter()
            .any(|issue| matches!(issue, PlanIssue::DuplicateLabel { .. }))
    );
    assert!(
        issues
            .iter()
            .any(|issue| matches!(issue, PlanIssue::UnresolvedEdge { .. }))
    );
    let pure_cycle = plan_with(vec![
        todo("a", &["b"], TodoState::Pending)?,
        todo("b", &["a"], TodoState::Pending)?,
    ])?;
    assert!(
        pure_cycle
            .validate()
            .iter()
            .any(|issue| matches!(issue, PlanIssue::Cycle { .. }))
    );
    Ok(())
}

#[test]
fn frontmatter_round_trips() -> TestResult {
    let mut plan = plan_with(vec![
        todo(
            "a",
            &[],
            TodoState::Done {
                output: Some("kernel://main/seam".parse()?),
            },
        )?,
        todo(
            "b",
            &["a"],
            TodoState::Blocked {
                on: BlockedOn::External { probe: None },
                note: "waiting".to_owned(),
            },
        )?,
    ])?;
    plan.tier = PlanTier::Sub {
        parent: TodoAddr {
            plan: PlanId::new("root")?,
            todo: TodoLabel::new("Some parent todo")?,
        },
    };
    let json = serde_json::to_string(&plan)?;
    assert!(json.contains("\"format\":2"));
    assert!(json.contains("\"parent\":\"root/Some parent todo\""));
    let back: Plan = serde_json::from_str(&json)?;
    assert_eq!(back, plan);
    let stray = json.replace("\"state\":\"blocked\"", "\"state\":\"pending\"");
    assert!(serde_json::from_str::<Plan>(&stray).is_err());
    Ok(())
}

#[test]
fn unknown_tags_round_trip_byte_identically() -> TestResult {
    let json = concat!(
        "{\"format\":2,\"plan\":\"p\",\"goal\":\"g\",\"version\":1,",
        "\"touched\":1,\"tier\":\"quarantine\",\"state\":\"parked\",",
        "\"intent\":null,\"constraints\":[],\"examples\":[],\"shape\":null,\"placement\":null,",
        "\"todos\":[{\"label\":\"a\",\"state\":\"quarantined\",\"attempt\":1,\"refusals\":0}]}"
    );
    let plan: Plan = serde_json::from_str(json)?;
    assert_eq!(plan.state, PlanState::Other("parked".to_owned()));
    assert_eq!(
        plan.todos.first().map(|todo| &todo.state),
        Some(&TodoState::Other("quarantined".to_owned()))
    );
    assert_eq!(serde_json::to_string(&plan)?, json);

    let blocked: BlockedOn = serde_json::from_str("\"quarantined\"")?;
    assert_eq!(blocked, BlockedOn::Other("quarantined".to_owned()));
    let isolation: Isolation = serde_json::from_str("\"vm\"")?;
    assert_eq!(isolation, Isolation::Other("vm".to_owned()));
    let check: Check = serde_json::from_str("\"just look\"")?;
    assert_eq!(check, Check::Other("just look".to_owned()));
    Ok(())
}

#[test]
fn external_probe_matches_the_fixture_wire_form() -> TestResult {
    let wire = "{\"external\":{\"probe\":\"test -e /app/vendor/zstd\"}}";
    let on: BlockedOn = serde_json::from_str(wire)?;
    assert_eq!(
        on,
        BlockedOn::External {
            probe: Some(ProbeCommand::new("test -e /app/vendor/zstd")?),
        }
    );
    assert_eq!(serde_json::to_string(&on)?, wire);
    assert!(ProbeCommand::new(" ").is_err());
    assert!(ProbeCommand::new("a\nb").is_err());
    Ok(())
}

#[test]
fn todo_addr_urls_use_the_slug() -> TestResult {
    let addr = TodoAddr {
        plan: PlanId::new("ship-logrotate-lite-with-a-packaged")?,
        todo: TodoLabel::new("Implement refresh flow")?,
    };
    assert_eq!(
        addr.to_url()?.to_string(),
        "plan://ship-logrotate-lite-with-a-packaged/implement-refresh-flow"
    );
    assert_eq!(
        String::from(addr),
        "ship-logrotate-lite-with-a-packaged/Implement refresh flow"
    );

    let colliding = plan_with(vec![
        todo("Fix the bug!", &[], TodoState::Pending)?,
        todo("Fix the bug?", &[], TodoState::Pending)?,
    ])?;
    assert!(
        colliding
            .validate()
            .iter()
            .any(|issue| matches!(issue, PlanIssue::SlugCollision { .. }))
    );
    Ok(())
}

#[test]
fn spawns_only_charges_upward_and_skips_when_zero() -> TestResult {
    let mut plan = plan_with(vec![todo("a", &[], TodoState::Pending)?])?;
    assert!(!serde_json::to_string(&plan)?.contains("spawns"));
    plan.charge_spawn();
    plan.charge_spawn();
    assert_eq!(plan.spawns().get(), 2);
    assert!(plan.spawns() < SPAWN_CAP);
    let json = serde_json::to_string(&plan)?;
    assert!(json.contains("\"spawns\":2"));
    let back: Plan = serde_json::from_str(&json)?;
    assert_eq!(back.spawns(), plan.spawns());
    Ok(())
}

#[test]
fn an_abandoned_predecessor_clears_its_edge_and_a_failed_one_does_not() -> TestResult {
    let plan = plan_with(vec![
        todo("dropped", &[], TodoState::Abandoned)?,
        todo("after the drop", &["dropped"], TodoState::Pending)?,
        todo(
            "flunked",
            &[],
            TodoState::Failed {
                cause: "the probe disagreed".to_owned(),
                last: None,
            },
        )?,
        todo("after the failure", &["flunked"], TodoState::Pending)?,
    ])?;
    let ready: Vec<&str> = plan
        .ready()
        .iter()
        .map(|todo| todo.label.as_str())
        .collect();
    assert_eq!(ready, vec!["after the drop"]);
    Ok(())
}

#[test]
fn a_terminal_record_keeps_the_owners_kernel_and_refuses_a_childs() -> TestResult {
    let owner = AgentId::new("main")?;
    for (url, expected) in [
        ("kernel://token_api_seam", Durability::Durable),
        ("kernel://main/cli_surface", Durability::Durable),
        ("kernel://write-the-patch/scratch", Durability::Ephemeral),
        ("agent://main", Durability::Ephemeral),
        ("history://main/e7", Durability::Durable),
        ("local://seam.md", Durability::Durable),
    ] {
        assert_eq!(
            terminal_durability(&url.parse::<Url>()?, &owner),
            expected,
            "{url}"
        );
    }
    Ok(())
}

#[test]
fn children_round_trip_and_stay_out_of_a_flat_row() -> TestResult {
    let mut parent = todo("parent", &[], TodoState::Pending)?;
    parent.children = vec![todo("child", &[], TodoState::Done { output: None })?];
    let flat = todo("flat", &[], TodoState::Pending)?;
    let plan = plan_with(vec![parent, flat])?;
    let json = serde_json::to_string(&plan)?;
    assert!(json.contains("\"children\":[{"), "{json}");
    assert_eq!(
        json.matches("\"children\"").count(),
        1,
        "an empty list is not written"
    );
    let back: Plan = serde_json::from_str(&json)?;
    assert_eq!(back, plan);
    let progress = yi_types::plan::doc::progress(&back.todos);
    assert_eq!((progress.done, progress.total), (1, 3));
    Ok(())
}

#[test]
fn a_format_one_document_reads_only_through_the_legacy_parser() -> TestResult {
    let json = concat!(
        "{\"format\":1,\"plan\":\"p\",\"goal\":\"g\",\"version\":1,",
        "\"tier\":\"root\",\"state\":\"active\",",
        "\"todos\":[{\"label\":\"a\",\"state\":\"pending\"}]}"
    );
    assert!(
        serde_json::from_str::<Plan>(json).is_err(),
        "format 1 is refused after import"
    );
    let plan = Plan::parse_legacy(json)?;
    assert_eq!(plan.todos.first().map(|todo| todo.attempt.get()), Some(1));
    assert!(Plan::parse_legacy(&json.replace("\"format\":1", "\"format\":2")).is_err());
    Ok(())
}
