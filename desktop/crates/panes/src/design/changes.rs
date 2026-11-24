//! The list of design changes: read from the page's feed, shown in the card, handed to the
//! agent as one prompt with each changed element and its source attached.

use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::tools::design::{CHANGE, Property};
use yd_core::feed::Table;
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::row::{Row, Rows};

/// The most characters the card shows of what an element said and says.
const MAX_SAID: usize = 24;

/// One property of one element that is not as the page drew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The element's number on its page and its selector-style name.
    pub number: u32,
    pub element: String,
    /// The page it is on, as its address names it, and where the page says it is drawn.
    pub page: String,
    pub source: String,
    pub property: Property,
    pub was: String,
    pub now: String,
}

fn read(row: &Row) -> Option<Change> {
    let rest = row.id.as_str().strip_prefix(CHANGE)?;
    let (number, slug) = rest.split_once(':')?;
    let word = |cell: &str| row.words(cell).unwrap_or_default().to_owned();
    Some(Change {
        number: number.parse().ok()?,
        element: word("element"),
        page: word("page"),
        source: word("source"),
        property: Property::from_slug(slug)?,
        was: word("was"),
        now: word("now"),
    })
}

/// The changes the page's feed holds, by element and then in the pane's order of properties.
pub fn list(table: Option<&Table>) -> Vec<Change> {
    let rows = table
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|at| t.get(at)));
    let mut changes: Vec<Change> = rows.filter_map(read).collect();
    changes.sort_by_key(|change| (change.number, change.property));
    changes
}

/// "4 changes", "1 change".
pub fn count(changes: usize) -> String {
    let noun = if changes == 1 { "change" } else { "changes" };
    format!("{changes} {noun}")
}

/// What an element says, short enough for one line of the card: its first sentence.
fn said(words: &str) -> String {
    let end = words.find(['.', '!', '?']).unwrap_or(words.len());
    let first = words.get(..end).unwrap_or(words);
    let short: String = first.chars().take(MAX_SAID).collect();
    let cut = short.len() < words.len();
    format!("\"{short}{}\"", if cut { "…" } else { "" })
}

impl Change {
    /// The card's words for it: `#fdecec → #fff4e5`, `"Too many uploads…" → "Slow down…"`.
    pub fn line(&self) -> String {
        match self.property {
            Property::Says => format!("{} → {}", said(&self.was), said(&self.now)),
            Property::Size
            | Property::Padding
            | Property::Fill
            | Property::Text
            | Property::Corners => format!("{} → {}", self.was, self.now),
        }
    }

    /// `yi://element/<page>#<name>`; none for a page with no address.
    pub fn address(&self) -> Option<Address> {
        element(&self.page, &self.element)
    }
}

pub fn element(page: &str, name: &str) -> Option<Address> {
    Some(Address::new(Kind::Element, page).ok()?.part(name))
}

/// Where a source location is, as lines of a file under `folder`: `src/pages/Upload.tsx:41`
/// is line 41. None for the dash a page with no source location gives.
pub fn lines(source: &str, folder: Option<&str>) -> Option<Address> {
    let (path, line) = source.rsplit_once(':')?;
    let line: u32 = line.parse().ok()?;
    let under = folder.and_then(|folder| path.strip_prefix(folder));
    let path = under.map_or(path, |rest| rest.trim_start_matches('/'));
    let file = Address::new(Kind::Lines, path).ok()?;
    Some(file.part(format!("L{line}")))
}

/// The one prompt that hands the changes over: every change in full, under its element.
pub fn prompt(changes: &[Change]) -> String {
    let mut text = String::from(
        "Write these design changes into the code. They are live in the preview only.\n",
    );
    let mut under = None;
    for change in changes {
        if under != Some(change.number) {
            under = Some(change.number);
            let (name, page, source) = (&change.element, &change.page, &change.source);
            text.push_str(&format!("\n{name} on {page}, drawn at {source}\n"));
        }
        let (noun, was, now) = (change.property.noun(), &change.was, &change.now);
        let line = match change.property {
            Property::Says => format!("- {noun}: \"{was}\" → \"{now}\"\n"),
            Property::Size
            | Property::Padding
            | Property::Fill
            | Property::Text
            | Property::Corners => format!("- {noun}: {was} → {now}\n"),
        };
        text.push_str(&line);
    }
    text
}

/// What goes with the prompt: each changed element, then the lines that draw it when the
/// page says where. `folder` is the thread's, so a path under it is the repository's.
pub fn things(model: &Model, changes: &[Change], folder: Option<&str>) -> Vec<Thing> {
    let mut things: Vec<Thing> = Vec::new();
    for change in changes {
        let at = [change.address(), lines(&change.source, folder)];
        for address in at.into_iter().flatten() {
            if !things.iter().any(|thing| thing.address == address) {
                things.push(Thing::at(model, address));
            }
        }
    }
    things
}
