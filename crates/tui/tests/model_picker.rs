mod common;

use serde_json::json;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::{KeyCodeValue, SingleKey};
use yi_tui::model::{ModelPopup, advanced_hint, cycle_efforts, step_effort};
use yi_tui::popup::{BottomView, PopupResult};
use yi_types::model::{Effort, Model};

fn reasoning(id: &str, map: Option<serde_json::Value>) -> Model {
    let mut model = common::test_model(id);
    model.reasoning = true;
    model.thinking_level_map = map;
    model
}

fn key(code: KeyCodeValue) -> SingleKey {
    SingleKey {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

fn text(popup: &ModelPopup) -> String {
    let theme = Theme::new(ColorTier::TrueColor, true);
    popup
        .lines(80, &theme)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_cycle_never_walks_into_the_advanced_tiers() {
    let model = reasoning("m", Some(json!({"xhigh": "xhigh", "max": "max"})));
    assert!(model.supported_efforts().contains(&Effort::Max));
    assert!(
        !cycle_efforts(&model)
            .iter()
            .any(|effort| effort.is_advanced())
    );
    assert_eq!(step_effort(&model, Effort::High, true), None);
}

#[test]
fn a_bounded_raise_names_where_the_advanced_tiers_live() {
    let model = reasoning("m", Some(json!({"xhigh": "xhigh", "max": "max"})));
    let hint = advanced_hint(&model).unwrap_or_default();
    assert!(hint.contains("xhigh"), "{hint}");
    assert!(hint.contains("max"), "{hint}");
    assert!(hint.contains("More reasoning"), "{hint}");
}

#[test]
fn a_model_without_advanced_tiers_offers_no_hint() {
    assert_eq!(advanced_hint(&reasoning("m", None)), None);
}

#[test]
fn stepping_walks_the_advertised_ladder_in_both_directions() {
    let model = reasoning("m", None);
    assert_eq!(
        step_effort(&model, Effort::Medium, true),
        Some(Effort::High)
    );
    assert_eq!(
        step_effort(&model, Effort::Medium, false),
        Some(Effort::Low)
    );
    assert_eq!(step_effort(&model, Effort::Off, false), None);
}

#[test]
fn an_unadvertised_current_anchors_instead_of_guessing() {
    let model = reasoning(
        "m",
        Some(json!({"minimal": null, "low": null, "medium": null})),
    );
    assert_eq!(cycle_efforts(&model), vec![Effort::Off, Effort::High]);
    assert_eq!(step_effort(&model, Effort::Low, true), None);
    assert_eq!(step_effort(&model, Effort::Low, false), Some(Effort::Off));
}

#[test]
fn picking_a_model_opens_its_effort_list() {
    let model = reasoning("m", None);
    let mut popup = ModelPopup::new(vec![model.clone()], &model, Effort::Medium, &[]);
    assert!(text(&popup).contains("faux/m"));
    assert!(matches!(
        popup.handle_key(&key(KeyCodeValue::Enter)),
        PopupResult::Open
    ));
    let shown = text(&popup);
    assert!(shown.contains("reasoning for faux/m"), "{shown}");
    assert!(shown.contains("medium"), "{shown}");
    assert!(!shown.contains("More reasoning"), "{shown}");
}

#[test]
fn the_advanced_tiers_need_the_second_step() {
    let model = reasoning("m", Some(json!({"xhigh": "xhigh", "max": "max"})));
    let mut popup = ModelPopup::new(vec![model.clone()], &model, Effort::Medium, &[]);
    popup.handle_key(&key(KeyCodeValue::Enter));
    let shown = text(&popup);
    assert!(shown.contains("More reasoning"), "{shown}");
    assert!(!shown.contains("xhigh"), "{shown}");

    for _ in 0..8 {
        popup.handle_key(&key(KeyCodeValue::Down));
    }
    popup.handle_key(&key(KeyCodeValue::Enter));
    let shown = text(&popup);
    assert!(shown.contains("xhigh"), "{shown}");
    assert!(shown.contains("max"), "{shown}");

    popup.handle_key(&key(KeyCodeValue::Enter));
    assert_eq!(
        popup.chosen.map(|(_, effort)| effort),
        Some(Effort::XHigh),
        "the first advanced row is the weaker tier"
    );
}

#[test]
fn a_single_rung_model_skips_the_effort_step() {
    let model = common::test_model("plain");
    let mut popup = ModelPopup::new(vec![model.clone()], &model, Effort::Medium, &[]);
    assert!(matches!(
        popup.handle_key(&key(KeyCodeValue::Enter)),
        PopupResult::Close
    ));
    assert_eq!(popup.chosen.map(|(_, effort)| effort), Some(Effort::Off));
}

#[test]
fn typing_filters_and_the_current_model_is_marked() {
    let a = reasoning("alpha", None);
    let b = reasoning("beta", None);
    let mut popup = ModelPopup::new(vec![a.clone(), b], &a, Effort::Medium, &[]);
    assert!(text(&popup).contains("› faux/alpha"));
    popup.handle_key(&key(KeyCodeValue::Char('b')));
    let shown = text(&popup);
    assert!(shown.contains("faux/beta"), "{shown}");
    assert!(!shown.contains("faux/alpha"), "{shown}");
}
