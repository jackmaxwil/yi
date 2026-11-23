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
        OPEN_FILE => open_file(model, &run.target),
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

/// Opens a file in the code viewer: its row in [`OPEN_FILES`] (id `file:<path>`, cells `path`,
/// `tab`, `opened`) becomes the newest, and the files source is asked for its text.
fn open_file(model: &mut Model, target: &Target) -> Vec<Effect> {
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
    let tab = kept.unwrap_or_else(fresh).text("opened", next.as_str());
    model.tables.apply(OPEN_FILES, FeedEvent::Upsert(tab));
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
