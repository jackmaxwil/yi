//! The pane's rows as data: the changed list, the tree under it, or the matches of a search.

use std::collections::BTreeMap;

use rune_kit::{TreeModel, natural_cmp};
use yd_core::feed::Table;
use yd_core::row::{Row, Rows};

use super::marks::Mark;
use super::{Filter, State};

/// A changed file as the `changes` feed tells it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Change {
    pub status: String,
    pub added: String,
    pub removed: String,
}

/// One drawn row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    Section {
        words: String,
        note: Option<&'static str>,
    },
    /// A file in the flat list on top, or a match of a search.
    Flat(String),
    Node {
        path: String,
        name: String,
        depth: u32,
        dir: bool,
        open: bool,
    },
}

/// What the pane shows, read from the two feeds and the thread's marks.
#[derive(Debug, Default)]
pub struct Data {
    pub items: Vec<Item>,
    pub marks: BTreeMap<String, Mark>,
    /// In the order the feed sent them.
    pub changes: Vec<(String, Change)>,
    pub files: usize,
    /// Why the tree stops short, when the source's walk was cut.
    pub cut: Option<String>,
}

impl Data {
    pub fn change(&self, path: &str) -> Option<&Change> {
        self.changes
            .iter()
            .find_map(|(p, change)| (p == path).then_some(change))
    }

    fn changed(&self, path: &str) -> bool {
        self.change(path).is_some()
    }

    pub fn touched(&self, path: &str) -> bool {
        self.changed(path)
            || self
                .marks
                .get(path)
                .is_some_and(|m| m.reads > 0 || m.edited)
    }

    fn edited(&self, path: &str) -> bool {
        self.changed(path) || self.marks.get(path).is_some_and(|m| m.edited)
    }

    /// A folder is touched when anything under it is.
    pub fn touched_under(&self, folder: &str) -> bool {
        let inside = |path: &String| {
            path.strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('/'))
        };
        self.changes
            .iter()
            .map(|(p, _)| p)
            .chain(self.marks.keys())
            .any(|path| inside(path) && self.touched(path))
    }

    /// Whether a file stays bright under `filter`.
    pub fn bright(&self, path: &str, filter: Filter) -> bool {
        match filter {
            Filter::All => true,
            Filter::Touched => self.touched(path),
            Filter::Edited => self.edited(path),
        }
    }

    pub fn bright_under(&self, folder: &str, filter: Filter) -> bool {
        let inside = |path: &String| {
            path.strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('/'))
        };
        let mut paths = self.changes.iter().map(|(p, _)| p).chain(self.marks.keys());
        filter == Filter::All || paths.any(|path| inside(path) && self.bright(path, filter))
    }

    pub fn count(&self, filter: Filter) -> usize {
        let paths = self
            .changes
            .iter()
            .map(|(p, _)| p)
            .chain(self.marks.keys().filter(|p| !self.changed(p)));
        let keep = |path: &&String| match filter {
            Filter::All | Filter::Touched => self.touched(path),
            Filter::Edited => self.edited(path),
        };
        paths.filter(keep).count()
    }
}

/// Reads the tables into rows to draw.
pub fn data(
    files: Option<&Table>,
    changes: Option<&Table>,
    marks: BTreeMap<String, Mark>,
    state: &State,
) -> Data {
    let mut data = Data {
        marks,
        ..Data::default()
    };
    for row in changes
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|i| t.get(i)))
    {
        if let Some(path) = row.id.as_str().strip_prefix("change:") {
            let cell = |field| row.words(field).unwrap_or("0").to_owned();
            let change = Change {
                status: cell("status"),
                added: cell("added"),
                removed: cell("removed"),
            };
            data.changes.push((path.to_owned(), change));
        }
    }
    let rows = files
        .into_iter()
        .flat_map(|t| (0..t.len()).filter_map(|i| t.get(i)));
    let (nodes, rest): (Vec<&Row>, Vec<&Row>) = rows.partition(|row| row.get("kind").is_some());
    data.cut = rest
        .iter()
        .find(|row| row.id.as_str() == "tree")
        .and_then(|row| row.words("cut"))
        .map(|why| ["[…] ", why].concat());
    data.files = nodes
        .iter()
        .filter(|row| row.words("kind") == Some("file"))
        .count();
    let query = state.query().map(str::to_lowercase);
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        let hit = |row: &&&Row| {
            row.words("kind") == Some("file") && name(row).to_lowercase().contains(&query)
        };
        let hits: Vec<String> = nodes
            .iter()
            .filter(hit)
            .map(|row| row.id.as_str().to_owned())
            .collect();
        let count = if hits.len() == 1 {
            "1 match".to_owned()
        } else {
            format!("{} matches", hits.len())
        };
        data.items.push(section(count, None));
        data.items.extend(hits.into_iter().map(Item::Flat));
        return data;
    }
    data.items.push(section(
        format!("Changed in this thread · {}", data.changes.len()),
        None,
    ));
    data.items.extend(
        data.changes
            .iter()
            .map(|(path, _)| Item::Flat(path.clone())),
    );
    data.items.push(section(
        "Everything".to_owned(),
        Some("touched stays bright"),
    ));
    let tree = tree(&nodes, &data, state);
    for row in 0..tree.visible_len() {
        if let Some((node, at)) = tree.row(row) {
            let open = at.expanded == Some(true);
            data.items.push(Item::Node {
                path: node.0.clone(),
                name: node.1.clone(),
                depth: at.depth,
                dir: node.2,
                open,
            });
        }
    }
    data
}

fn section(words: String, note: Option<&'static str>) -> Item {
    Item::Section { words, note }
}

fn name(row: &Row) -> &str {
    row.words("name").unwrap_or(row.id.as_str())
}

/// Path, name and whether it is a folder.
type Node = (String, String, bool);

/// The tree, folders first, a folder open when it holds something touched unless toggled.
fn tree(nodes: &[&Row], data: &Data, state: &State) -> TreeModel<Node> {
    let mut sorted = nodes.to_vec();
    sorted.sort_by_key(|row| row.id.as_str().matches('/').count());
    let mut model = TreeModel::new();
    for row in sorted {
        let dir = row.words("kind") == Some("folder");
        let parent = row.parent.as_ref().map(|p| p.as_str());
        model.insert(
            parent,
            row.id.as_str(),
            (row.id.as_str().to_owned(), name(row).to_owned(), dir),
        );
    }
    // Folders first, then names; dot-names after the rest, as the mockup draws `.github`.
    let rank = |node: &Node| (!node.2, node.1.starts_with('.'));
    model.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| natural_cmp(&a.1, &b.1)));
    for (path, _, dir) in nodes
        .iter()
        .map(|row| (row.id.as_str(), (), row.words("kind") == Some("folder")))
    {
        if dir {
            model.set_expanded(
                path,
                data.touched_under(path) != state.toggled.contains(path),
            );
        }
    }
    model
}
