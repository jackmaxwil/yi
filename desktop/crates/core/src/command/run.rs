//! What each model-routed command does to the model.

use std::path::PathBuf;

use crate::address::attach::{self, ATTACH};
use crate::command::by_kind::{FINDING, NEW_FINDING};
use crate::command::table::{FLIP_CHAT, OPEN_FILE, Run, Target};
use crate::feed::{FILES, FeedEvent, OPEN_FILES, SourceCmd, Table};
use crate::findings::triage;
use crate::ids::{CommandId, RowId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::Effect;
use crate::row::{Row, Rows};

/// The model-routed commands [`run`] carries out; any other changes nothing here.
const CARRIED: &[CommandId] = &[FLIP_CHAT, OPEN_FILE, ATTACH, FINDING, NEW_FINDING];

/// Whether [`run`] carries `id` out: a model command it lacks is listed but not built.
pub fn carries(id: CommandId) -> bool {
    CARRIED.contains(&id)
}

/// Carries out a command routed to the model. An id this table lacks changes nothing.
pub fn run(model: &mut Model, run: &Run) -> Vec<Effect> {
    match run.command {
        FLIP_CHAT => {
            model.grid.side = model.grid.side.flipped();
            Vec::new()
        }
        OPEN_FILE => open_file(model, &run.target, run.text.as_deref() == Some(PREVIEW)),
        ATTACH => attach::run(model, run),
        // A finding is made of a thing: aimed at nothing, the command makes none.
        FINDING | NEW_FINDING => {
            if let Target::Thing(evidence) = &run.target {
                triage::make(model, None, evidence.clone());
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// How many opens an open file's row has seen: the highest is the file that shows.
pub fn opened(tab: &Row) -> u64 {
    let count = tab.words("opened").and_then(|count| count.parse().ok());
    count.unwrap_or(0)
}

fn newest(table: &Table) -> u64 {
    let tabs = (0..table.len()).filter_map(|at| table.get(at));
    tabs.map(opened).max().unwrap_or(0)
}

/// The text of a `code.open` that only looks: a file not open yet opens as the one preview tab.
pub const PREVIEW: &str = "preview";

/// Whether an open file's row is the preview tab: opened by a look, and kept by no one yet.
pub fn previewed(tab: &Row) -> bool {
    tab.words(PREVIEW) == Some("1")
}

/// Opens a file in the code viewer: its row in [`OPEN_FILES`] (id `file:<path>`, cells `path`,
/// `tab`, `opened`, `preview`) becomes the newest, and the files source is asked for its text.
fn open_file(model: &mut Model, target: &Target, look: bool) -> Vec<Effect> {
    let Target::Thing(file) = target else {
        return Vec::new();
    };
    if !matches!(file.kind(), Kind::File | Kind::Lines) {
        return Vec::new();
    }
    let id = RowId::new(format!("file:{}", file.id()));
    let table = model.tables.get(OPEN_FILES);
    let next = table.map_or(0, newest).saturating_add(1).to_string();
    let kept = table.and_then(|table| table.find(&id)).cloned();
    // Invariant: counts only rise, so a row's first count names its tab for good.
    let fresh = || {
        Row::new(id.as_str())
            .text("path", file.id())
            .text("tab", next.as_str())
    };
    // A look at a file not open takes the last look's tab; at an open one it changes nothing.
    let (first, mark) = (kept.is_none(), kept.as_ref().is_some_and(previewed));
    let passing: Vec<RowId> = match table.filter(|_| look && first) {
        Some(table) => {
            let tabs = (0..table.len()).filter_map(|at| table.get(at));
            tabs.filter(|tab| previewed(tab))
                .map(|tab| tab.id.clone())
                .collect()
        }
        None => Vec::new(),
    };
    let mark = if look && (first || mark) { "1" } else { "" };
    let tab = kept.unwrap_or_else(fresh).text("opened", next.as_str());
    for old in passing {
        model.tables.apply(OPEN_FILES, FeedEvent::Remove(old));
    }
    model
        .tables
        .apply(OPEN_FILES, FeedEvent::Upsert(tab.text(PREVIEW, mark)));
    vec![Effect::Source(
        FILES,
        SourceCmd::Read(PathBuf::from(file.id())),
    )]
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;

    use super::*;
    use crate::address::uri::Address;
    use crate::command::table::Route;
    use crate::msg::{Msg, step};

    fn open(model: &mut Model, address: &str) -> Result<Vec<Effect>, Box<dyn Error>> {
        let run = Run::new(OPEN_FILE).on(Target::Thing(address.parse::<Address>()?));
        Ok(step(model, Msg::Command(Route::Model, run)))
    }

    #[test]
    fn opening_a_file_adds_its_tab_and_asks_the_files_source_for_its_text()
    -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        let asked = open(&mut model, "yi://file/src/routes/upload.rs")?;
        let read = SourceCmd::Read(Path::new("src/routes/upload.rs").to_owned());
        assert_eq!(asked, [Effect::Source(FILES, read)]);
        open(&mut model, "yi://lines/src/lib.rs#L13")?;
        open(&mut model, "yi://file/src/routes/upload.rs")?;
        let table = model.tables.get(OPEN_FILES).ok_or("no open files")?;
        assert_eq!(table.len(), 2, "a file opened twice keeps one tab");
        let upload = table
            .find(&RowId::new("file:src/routes/upload.rs"))
            .ok_or("no tab for upload.rs")?;
        assert_eq!((upload.words("tab"), opened(upload)), (Some("1"), 3));
        let lib = table
            .find(&RowId::new("file:src/lib.rs"))
            .ok_or("lines of a file open the file")?;
        assert_eq!((lib.words("tab"), opened(lib)), (Some("2"), 2));
        Ok(())
    }

    fn look(model: &mut Model, address: &str) -> Result<Vec<Effect>, Box<dyn Error>> {
        let run = Run {
            text: Some(PREVIEW.to_owned()),
            ..Run::new(OPEN_FILE).on(Target::Thing(address.parse::<Address>()?))
        };
        Ok(step(model, Msg::Command(Route::Model, run)))
    }

    /// The open files as `(path, is the preview)`, in tab order.
    fn files(model: &Model) -> Vec<(String, bool)> {
        let Some(table) = model.tables.get(OPEN_FILES) else {
            return Vec::new();
        };
        let tabs = (0..table.len()).filter_map(|at| table.get(at));
        let file = |tab: &Row| {
            (
                tab.words("path").unwrap_or_default().to_owned(),
                previewed(tab),
            )
        };
        tabs.map(file).collect()
    }

    /// Three files looked at leave one tab, the last; a file opened stays, and the next look
    /// goes beside it. Each look still asks the files source for the text.
    #[test]
    fn a_look_takes_the_last_looks_tab_and_an_opened_file_stays() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        for path in ["a.rs", "b.rs", "c.rs"] {
            let asked = look(&mut model, &format!("yi://file/{path}"))?;
            assert_eq!(asked.len(), 1, "{path} is read");
        }
        assert_eq!(files(&model), [("c.rs".to_owned(), true)]);
        open(&mut model, "yi://file/c.rs")?;
        assert_eq!(files(&model), [("c.rs".to_owned(), false)], "opened, kept");
        look(&mut model, "yi://file/d.rs")?;
        look(&mut model, "yi://file/e.rs")?;
        let kept = [("c.rs".to_owned(), false), ("e.rs".to_owned(), true)];
        assert_eq!(files(&model), kept);
        Ok(())
    }

    /// A look at a file that is open brings it in front and changes no tab: a kept file stays
    /// kept, the preview stays the preview, and neither closes the other.
    #[test]
    fn a_look_at_an_open_file_changes_no_tab() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        open(&mut model, "yi://file/a.rs")?;
        look(&mut model, "yi://file/b.rs")?;
        let both = [("a.rs".to_owned(), false), ("b.rs".to_owned(), true)];
        look(&mut model, "yi://file/a.rs")?;
        assert_eq!(files(&model), both, "a kept file looked at stays kept");
        let table = model.tables.get(OPEN_FILES).ok_or("no open files")?;
        let front = (0..table.len()).filter_map(|at| table.get(at));
        let front = front.max_by_key(|tab| opened(tab)).ok_or("no tab")?;
        assert_eq!(front.words("path"), Some("a.rs"), "and is in front");
        look(&mut model, "yi://file/b.rs")?;
        assert_eq!(
            files(&model),
            both,
            "the preview looked at again is still it"
        );
        Ok(())
    }

    #[test]
    fn a_command_not_built_is_listed_and_changes_nothing() -> Result<(), Box<dyn Error>> {
        use crate::command::by_kind::PUT_BACK;
        use crate::command::table::{Commands, declared};
        let table = Commands::new(declared());
        let listed = table.find(PUT_BACK).ok_or("put back is not listed")?;
        assert!(!Commands::built(listed, |_| false));
        let mut model = Model::default();
        let lines = Target::Thing("yi://lines/src/lib.rs#L3".parse::<Address>()?);
        let asked = step(
            &mut model,
            Msg::Command(Route::Model, Run::new(PUT_BACK).on(lines)),
        );
        assert_eq!(asked, []);
        assert!(carries(OPEN_FILE) && !carries(PUT_BACK));
        Ok(())
    }

    #[test]
    fn a_thing_that_is_not_a_file_opens_nothing() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        assert_eq!(open(&mut model, "yi://thread/abc")?, []);
        assert!(model.tables.get(OPEN_FILES).is_none_or(|t| t.len() == 0));
        Ok(())
    }
}
