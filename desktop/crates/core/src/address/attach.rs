//! The things attached to the next prompt: at most twelve, numbered in the order added.

use serde_json::{Value, json};

use crate::address::thing::Thing;
use crate::address::uri::Address;
use crate::command::table::{Command, Route, Run, Target};
use crate::ids::CommandId;
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::Effect;

/// The most things one prompt carries.
pub const MAX_ATTACHED: usize = 12;
/// The most of a thing's excerpt a prompt carries, in bytes: what is past it is not sent.
pub const MAX_EXCERPT: usize = 8 * 1024;

/// Attaches the thing it is aimed at to the next prompt, or takes it off again.
pub const ATTACH: CommandId = CommandId("context.attach");
/// Starts picking things with the pointer, or stops.
pub const PICK: CommandId = CommandId("context.pick");

/// The commands of attaching. `context.attach` can be aimed at any kind of thing.
pub const COMMANDS: &[Command] = &[
    Command {
        id: ATTACH,
        title: "Attach to Chat",
        key: None,
        menu: None,
        kinds: Kind::ALL,
        route: Route::Model,
    },
    Command {
        id: PICK,
        title: "Pick Something to Attach",
        key: Some("alt+cmd+p"),
        menu: Some("View"),
        kinds: &[],
        route: Route::Model,
    },
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attached {
    pub things: Vec<Thing>,
}

impl Attached {
    /// The number a thing wears on its chip and its box, from 1: its place in the order added.
    pub fn number(&self, address: &Address) -> Option<usize> {
        let at = self.things.iter().position(|t| &t.address == address)?;
        Some(at.saturating_add(1))
    }

    /// Whether one more thing would be refused.
    pub fn is_full(&self) -> bool {
        self.things.len() >= MAX_ATTACHED
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttachMsg {
    /// Attach it, or take it off if it is already attached.
    Toggle(Thing),
    Clear,
}

/// Attaches, detaches or clears. A thirteenth thing changes nothing.
pub fn step(attached: &mut Attached, change: AttachMsg) -> Vec<Effect> {
    match change {
        AttachMsg::Toggle(thing) => match attached.number(&thing.address) {
            Some(_) => attached.things.retain(|t| t.address != thing.address),
            None if attached.is_full() => {}
            None => attached.things.push(thing),
        },
        AttachMsg::Clear => attached.things.clear(),
    }
    Vec::new()
}

/// Carries out `context.attach`: the thing at the address the command is aimed at is
/// attached under its kind's name, or taken off. Aimed at anything else it changes nothing.
pub fn run(model: &mut Model, run: &Run) -> Vec<Effect> {
    match (&run.target, run.command) {
        (Target::Thing(address), ATTACH) => {
            let thing = Thing::at(model, address.clone());
            step(&mut model.attached, AttachMsg::Toggle(thing))
        }
        (Target::Thing(_) | Target::App | Target::Pane(_), _) => Vec::new(),
    }
}

/// What a pane with no words of its own says it sent.
const NAME_ONLY: &str =
    "Only this pane's name reached this prompt: the app does not send what it shows as text.";

/// An excerpt held to [`MAX_EXCERPT`], with a notice at the cut naming what was kept.
fn capped(text: &str) -> String {
    let kept = text
        .get(..text.floor_char_boundary(MAX_EXCERPT))
        .unwrap_or(text);
    if kept.len() == text.len() {
        return text.to_owned();
    }
    let (kept_len, total) = (kept.len(), text.len());
    format!("{kept}\n[…] kept {kept_len} of {total} bytes of this thing (cap 8 KiB a thing)")
}

/// What the attached things add to a prompt: a file or lines as a link Yi's server reads, else
/// its name and excerpt. Incident: the model read a `yi://` address as a path and gave up.
pub fn blocks(attached: &[Thing]) -> Vec<Value> {
    let mut blocks = Vec::new();
    for thing in attached {
        let excerpt = thing.excerpt.as_deref().map(capped);
        if matches!(thing.kind(), Kind::File | Kind::Lines) {
            let (path, part) = (thing.address.id(), thing.address.part_text());
            let uri = part.map_or_else(|| path.to_owned(), |part| [path, "#", part].concat());
            blocks.push(json!({"type": "resource_link", "uri": uri, "name": thing.label}));
            if let Some(excerpt) = excerpt {
                blocks.push(json!({"type": "text", "text": format!("\n{excerpt}\n")}));
            }
            continue;
        }
        let excerpt = excerpt
            .or_else(|| matches!(thing.kind(), Kind::Pane | Kind::Tab).then(|| NAME_ONLY.into()));
        let said = excerpt.map(|text| format!("\n{text}")).unwrap_or_default();
        let page = (thing.kind() == Kind::Element).then(|| thing.address.id());
        let whose = page.map(|page| format!(", an element of the page at {page}"));
        let text = format!(
            "\nAttached: {}{}{said}\n",
            thing.label,
            whose.unwrap_or_default()
        );
        blocks.push(json!({"type": "text", "text": text}));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::msg::{Msg, step as step_model};

    type Check = Result<(), Box<dyn Error>>;

    fn thing(address: &str, label: &str, excerpt: Option<&str>) -> Result<Thing, Box<dyn Error>> {
        Ok(Thing {
            label: label.to_owned(),
            address: address.parse()?,
            excerpt: excerpt.map(str::to_owned),
        })
    }

    fn toggle(model: &mut Model, thing: &Thing) {
        step_model(model, Msg::Attach(AttachMsg::Toggle(thing.clone())));
    }

    #[test]
    fn a_thing_is_attached_once_numbered_in_order_and_toggled_off() -> Check {
        let mut model = Model::default();
        let lines = thing("yi://lines/src/lib.rs#L1", "lib.rs line 1", None)?;
        let alert = thing(
            "yi://element/localhost:5173/upload#div.alert",
            "div.alert",
            None,
        )?;
        let thread = thing("yi://thread/t1", "thread · One", None)?;
        for each in [&lines, &alert, &thread] {
            toggle(&mut model, each);
        }
        let number = |model: &Model, thing: &Thing| model.attached.number(&thing.address);
        assert_eq!(number(&model, &alert), Some(2));
        toggle(&mut model, &lines);
        assert_eq!(number(&model, &lines), None, "a second toggle takes it off");
        assert_eq!(number(&model, &alert), Some(1), "the rest are renumbered");
        assert_eq!(number(&model, &thread), Some(2));
        // The same address under another name is the same thing.
        let renamed = Thing {
            label: "the alert".to_owned(),
            ..alert.clone()
        };
        toggle(&mut model, &renamed);
        assert_eq!(model.attached.things, [thread]);
        step_model(&mut model, Msg::Attach(AttachMsg::Clear));
        assert!(model.attached.things.is_empty());
        Ok(())
    }

    #[test]
    fn a_thirteenth_thing_is_not_attached() -> Check {
        let mut model = Model::default();
        for n in 1..=13 {
            let line = thing(&format!("yi://lines/src/lib.rs#L{n}"), "a line", None)?;
            toggle(&mut model, &line);
        }
        assert_eq!(model.attached.things.len(), MAX_ATTACHED);
        assert!(model.attached.is_full());
        let last: Address = "yi://lines/src/lib.rs#L13".parse()?;
        assert_eq!(
            model.attached.number(&last),
            None,
            "the thirteenth is refused"
        );
        // A full tray still lets one go.
        let first = thing("yi://lines/src/lib.rs#L1", "a line", None)?;
        toggle(&mut model, &first);
        assert_eq!(model.attached.things.len(), 11);
        Ok(())
    }

    #[test]
    fn attach_is_a_command_aimed_at_a_thing() -> Check {
        let mut model = Model::default();
        let address: Address = "yi://lines/src/middleware/rate_limit.rs#L13-L20".parse()?;
        let attach = Run::new(ATTACH).on(Target::Thing(address.clone()));
        run(&mut model, &attach);
        let labels: Vec<&str> = model.attached.things.iter().map(|t| &*t.label).collect();
        assert_eq!(labels, ["rate_limit.rs lines 13–20"]);
        run(&mut model, &Run::new(ATTACH));
        run(
            &mut model,
            &Run::new(PICK).on(Target::Thing(address.clone())),
        );
        assert_eq!(model.attached.things.len(), 1, "aimed at nothing, nothing");
        run(&mut model, &attach);
        assert!(
            model.attached.things.is_empty(),
            "attach again takes it off"
        );
        Ok(())
    }

    #[test]
    fn files_go_as_links_and_everything_else_as_words() -> Check {
        let mut model = Model::default();
        let lines = thing(
            "yi://lines/src/middleware/rate_limit.rs#L13-L20",
            "rate_limit.rs lines 13–20",
            Some("pub fn take(&mut self) {}"),
        )?;
        let file = thing("yi://file/docs/a b.md", "file · a b.md", None)?;
        let alert = thing(
            "yi://element/localhost:5173/upload#div.alert",
            "div.alert",
            Some("Too many uploads. Try again in 41 seconds."),
        )?;
        let thread = thing("yi://thread/fork-1", "thread · Fork · sliding window", None)?;
        for each in [&lines, &file, &alert, &thread] {
            toggle(&mut model, each);
        }
        let sent = blocks(&model.attached.things);
        let want = json!([
            {"type": "resource_link", "uri": "src/middleware/rate_limit.rs#L13-L20",
                "name": "rate_limit.rs lines 13–20"},
            {"type": "text", "text": "\npub fn take(&mut self) {}\n"},
            {"type": "resource_link", "uri": "docs/a b.md", "name": "file · a b.md"},
            {"type": "text", "text": "\nAttached: div.alert, an element of the page at \
                localhost:5173/upload\nToo many uploads. Try again in 41 seconds.\n"},
            {"type": "text", "text": "\nAttached: thread · Fork · sliding window\n"},
        ]);
        assert_eq!(Value::Array(sent), want);
        Ok(())
    }

    #[test]
    fn an_excerpt_past_the_cap_is_cut_between_characters() -> Check {
        // 4,097 two-byte letters: the cap falls in the middle of the last one kept.
        let long = "é".repeat(MAX_EXCERPT / 2 + 1);
        let log = thing("yi://log-line/a1", "log line · a1", Some(&long))?;
        let sent = blocks(&[log]);
        let text = sent
            .first()
            .and_then(|block| block["text"].as_str())
            .ok_or("no text block")?;
        let head = "\nAttached: log line · a1\n";
        let kept = text.strip_prefix(head).ok_or("the name leads")?;
        let (kept, notice) = kept.split_once("\n[…]").ok_or("no notice at the cut")?;
        assert_eq!(kept.chars().count(), MAX_EXCERPT / 2);
        assert!(kept.chars().all(|c| c == 'é'));
        assert_eq!(
            notice.trim_end(),
            " kept 8192 of 8194 bytes of this thing (cap 8 KiB a thing)"
        );
        Ok(())
    }

    #[test]
    fn a_pane_that_sends_no_words_says_only_its_name_reached_the_prompt() -> Check {
        let code = thing("yi://pane/3", "pane · Code", None)?;
        let sent = blocks(&[code]);
        let text = sent
            .first()
            .and_then(|block| block["text"].as_str())
            .ok_or("no text block")?;
        assert_eq!(
            text,
            "\nAttached: pane · Code\nOnly this pane's name reached this prompt: the app does \
             not send what it shows as text.\n"
        );
        Ok(())
    }
}
