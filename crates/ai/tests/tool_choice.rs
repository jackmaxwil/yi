use serde_json::{Value, json};
use std::error::Error;
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::openai::{self, OpenAiOptions};
use yi_ai::openai_responses;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::{
    ForcedTool, FreeformFormat, LlmContext, Model, ModelCost, ToolChoice, ToolChoiceError, ToolDef,
};

type TestResult = Result<(), Box<dyn Error>>;

fn model() -> Model {
    let zero = || serde_json::Number::from(0);
    Model {
        id: "m".to_owned(),
        name: "m".to_owned(),
        api: "a".to_owned(),
        provider: "p".to_owned(),
        base_url: "https://example.invalid".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 1_000,
        max_tokens: 100,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn tool(name: &str, freeform: Option<FreeformFormat>) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        description: "d".to_owned(),
        parameters: json!({"type": "object", "properties": {}, "required": []}),
        freeform,
    }
}

fn bodies(choice: Option<ToolChoice>, tools: Vec<ToolDef>) -> [Value; 3] {
    let context = LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "s".to_owned(),
        messages: vec![AgentMessage::user_input(
            UserContent::Text("hi".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: Some(tools),
        tool_choice: choice,
    };
    [
        anthropic::build_params(&model(), &context, &AnthropicOptions::default()).into_value(),
        openai::build_params(&model(), &context, &OpenAiOptions::default()).into_value(),
        openai_responses::build_params(&model(), &context, &OpenAiOptions::default()).into_value(),
    ]
}

fn forced(name: &str) -> Result<ToolChoice, ToolChoiceError> {
    Ok(ToolChoice::Tool(ForcedTool::new(name)?))
}

#[test]
fn the_three_providers_spell_the_same_three_intents_differently() -> TestResult {
    let plan = || vec![tool("plan", None)];

    let [anthropic, completions, responses] = bodies(Some(ToolChoice::Auto), plan());
    assert_eq!(anthropic["tool_choice"], json!({"type": "auto"}));
    assert_eq!(completions["tool_choice"], json!("auto"));
    assert_eq!(responses["tool_choice"], json!("auto"));

    let [anthropic, completions, responses] = bodies(Some(ToolChoice::None), plan());
    assert_eq!(anthropic["tool_choice"], json!({"type": "none"}));
    assert_eq!(completions["tool_choice"], json!("none"));
    assert_eq!(responses["tool_choice"], json!("none"));

    let [anthropic, completions, responses] = bodies(Some(forced("plan")?), plan());
    assert_eq!(
        anthropic["tool_choice"],
        json!({"type": "tool", "name": "plan"})
    );
    assert_eq!(
        completions["tool_choice"],
        json!({"type": "function", "function": {"name": "plan"}})
    );
    assert_eq!(
        responses["tool_choice"],
        json!({"type": "function", "name": "plan"})
    );
    Ok(())
}

#[test]
fn a_forced_freeform_tool_is_custom_only_where_it_was_emitted_as_custom() -> TestResult {
    let scratch = vec![tool(
        "scratch",
        Some(FreeformFormat {
            definition: "start: /.*/".to_owned(),
        }),
    )];
    let [_, completions, responses] = bodies(Some(forced("scratch")?), scratch);
    assert_eq!(
        responses["tool_choice"],
        json!({"type": "custom", "name": "scratch"})
    );
    assert_eq!(
        completions["tool_choice"],
        json!({"type": "function", "function": {"name": "scratch"}})
    );
    Ok(())
}

#[test]
fn no_choice_emits_no_key_and_leaves_the_body_byte_identical() -> TestResult {
    let plan = || vec![tool("plan", None)];
    for (unforced, mut forced) in bodies(None, plan())
        .into_iter()
        .zip(bodies(Some(forced("plan")?), plan()))
    {
        assert!(unforced.get("tool_choice").is_none());
        assert!(forced.get("tool_choice").is_some());
        if let Value::Object(map) = &mut forced {
            map.remove("tool_choice");
        }
        assert_eq!(
            serde_json::to_string(&unforced)?,
            serde_json::to_string(&forced)?
        );
    }
    Ok(())
}

#[test]
fn a_forced_tool_switches_extended_thinking_off_for_that_turn() -> TestResult {
    let mut reasoning = model();
    reasoning.reasoning = true;
    let options = AnthropicOptions {
        thinking: anthropic::Thinking::Adaptive { effort: None },
        ..AnthropicOptions::default()
    };
    let context = |choice| LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "s".to_owned(),
        messages: vec![AgentMessage::user_input(
            UserContent::Text("hi".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: Some(vec![tool("plan", None)]),
        tool_choice: Some(choice),
    };
    let forced_turn = anthropic::build_params(&reasoning, &context(forced("plan")?), &options);
    assert_eq!(forced_turn["tool_choice"]["type"], json!("tool"));
    assert_eq!(forced_turn["thinking"], json!({"type": "disabled"}));
    let free_turn = anthropic::build_params(&reasoning, &context(ToolChoice::Auto), &options);
    assert_eq!(free_turn["thinking"]["type"], json!("adaptive"));
    Ok(())
}
