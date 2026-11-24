//! The theme library read, edited and written, on files under the lane's target.

use std::error::Error;
use std::path::PathBuf;

use yd_core::ids::ThemeId;
use yd_core::tokens::{Rgba, ThemeFile, ThemeValue, TokenError, Tokens};

use super::{FileError, Library, Origin, Refused};

const FIXTURES: [&str; 5] = [
    include_str!("../../../../../fixtures/mockup/themes/yi.json"),
    include_str!("../../../../../fixtures/mockup/themes/graphite.json"),
    include_str!("../../../../../fixtures/mockup/themes/ink.json"),
    include_str!("../../../../../fixtures/mockup/themes/terminal.json"),
    include_str!("../../../../../fixtures/mockup/themes/contrast.json"),
];

/// A folder of its own under the lane's target, emptied first.
fn folder(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/theme-files")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn each_built_in_theme_draws_the_values_the_mockup_gives_it() -> Result<(), Box<dyn Error>> {
    let library = Library::default();
    for text in FIXTURES {
        let drawn = ThemeFile::parse(text)?;
        let theme = library.theme(&drawn.id).ok_or("a theme is missing")?;
        assert_eq!(
            (theme.file.name.as_str(), theme.file.note.as_str()),
            (drawn.name.as_str(), drawn.note.as_str())
        );
        let mut want = Tokens::default();
        for (name, value) in &drawn.values {
            let (name, value) = match name.as_str() {
                "surface.pane" => ("glass.pane", value.clone()),
                "surface.pane_active" => ("glass.pane_in_use", value.clone()),
                "surface.window" => ("glass.window", value.clone()),
                "font.text" => match value {
                    ThemeValue::Text(css) if css.contains("Geist Mono") => {
                        ("font.text", ThemeValue::Text("Geist Mono".into()))
                    }
                    _ => ("font.text", ThemeValue::Text("system".into())),
                },
                other => (other, value.clone()),
            };
            want.set(name, &value)?;
        }
        let got = theme.tokens();
        assert_eq!(
            Tokens {
                window: got.window,
                ..want
            },
            got,
            "{}",
            drawn.id
        );
    }
    Ok(())
}

#[test]
fn a_light_theme_is_listed_but_cannot_be_put_in_use() {
    let mut library = Library::default();
    let paper = ThemeId::new("paper");
    assert!(
        library
            .theme(&paper)
            .is_some_and(|t| t.unavailable.is_some())
    );
    assert!(!library.pick(&paper));
    assert_eq!(library.active, ThemeId::new("yi"));
}

#[test]
fn a_value_that_does_not_fit_is_named_and_keeps_its_old_value() {
    let mut library = Library::default();
    let path = PathBuf::from("/themes/mine.json");
    library.take(
        &path,
        r##"{"id": "mine", "name": "Mine", "dark": true, "values": {"accent": "#112233"}}"##,
        None,
    );
    library.take(&path, r##"{"id": "mine", "name": "Mine", "dark": true, "values": {"accent": "blue", "ok": "#445566"}}"##, None);
    let mine = library.theme(&ThemeId::new("mine")).map(|t| t.tokens());
    assert_eq!(mine.as_ref().map(|t| t.accent), Rgba::parse("#112233"));
    assert_eq!(mine.as_ref().map(|t| t.ok), Rgba::parse("#445566"));
    assert_eq!(
        library
            .refusals
            .iter()
            .map(|r| r.what.clone())
            .collect::<Vec<_>>(),
        vec![Refused::Value {
            name: "accent".into(),
            why: TokenError::NotAColour
        }]
    );
}

#[test]
fn a_file_that_is_not_a_theme_keeps_what_was_read_from_it_before() {
    let mut library = Library::default();
    let path = PathBuf::from("/themes/mine.json");
    library.take(
        &path,
        r##"{"id": "mine", "name": "Mine", "dark": true, "values": {"accent": "#112233"}}"##,
        None,
    );
    library.take(&path, "{\"id\": \"mine\", \"na", None);
    let mine = library
        .theme(&ThemeId::new("mine"))
        .map(|t| t.tokens().accent);
    assert_eq!(mine, Rgba::parse("#112233"));
    assert_eq!(
        library.refusals.first().map(|r| r.what.clone()),
        Some(Refused::File(FileError::NotATheme))
    );
}

#[test]
fn a_file_may_not_take_a_built_in_themes_id() {
    let mut library = Library::default();
    let path = PathBuf::from("/themes/fake.json");
    library.take(
        &path,
        r##"{"id": "yi", "name": "Fake", "dark": true, "values": {"accent": "#ff0000"}}"##,
        None,
    );
    assert_eq!(library.tokens().accent, Tokens::default().accent);
    assert_eq!(
        library.refusals.first().map(|r| r.what.clone()),
        Some(Refused::File(FileError::IdTaken(ThemeId::new("yi"))))
    );
}

#[test]
fn a_file_changed_on_disk_is_read_again_and_a_removed_one_dropped() -> Result<(), Box<dyn Error>> {
    let dir = folder("changed")?;
    let file = dir.join("sea.json");
    std::fs::write(
        &file,
        r##"{"id": "sea", "name": "Sea", "dark": true, "values": {"accent": "#0000ff"}}"##,
    )?;
    let mut library = Library::at(dir.clone());
    let sea = ThemeId::new("sea");
    assert!(library.pick(&sea));
    assert_eq!(
        library.tokens().accent,
        Rgba::parse("#0000ff").ok_or("hex")?
    );
    std::fs::write(
        &file,
        r##"{"id": "sea", "name": "Sea", "dark": true, "values": {"accent": "#00ff00", "ok": "#010203"}}"##,
    )?;
    library.reload();
    assert_eq!(
        library.tokens().accent,
        Rgba::parse("#00ff00").ok_or("hex")?
    );
    std::fs::remove_file(&file)?;
    library.reload();
    assert!(library.theme(&sea).is_none());
    assert_eq!(
        library.active,
        ThemeId::new("yi"),
        "the base takes over from a theme that is gone"
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn editing_a_built_in_theme_writes_a_copy_that_reads_back() -> Result<(), Box<dyn Error>> {
    let dir = folder("copy")?;
    let mut library = Library::at(dir.clone());
    let ink = ThemeId::new("ink");
    let copy = library.set(&ink, "space.row", ThemeValue::Number(24.0))?;
    assert_eq!(copy, ThemeId::new("ink-copy"));
    assert_eq!(library.active, copy);
    assert_eq!(
        library.theme(&ink).map(|t| t.tokens().row),
        Some(20.0),
        "the built-in is not changed"
    );
    library.reload();
    let read = Library::at(dir.clone());
    let back = read.theme(&copy).ok_or("the copy was not written")?;
    assert_eq!(back.file.name, "Ink copy");
    assert_eq!(
        (back.tokens().row, back.tokens().accent),
        (
            24.0,
            library
                .theme(&ink)
                .map(|t| t.tokens().accent)
                .ok_or("ink")?
        )
    );
    assert!(matches!(back.origin, Origin::File { .. }));
    assert_eq!(
        library.set(&copy, "accent", ThemeValue::Number(3.0)),
        Err(TokenError::NotAColour)
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn a_theme_write_never_goes_through_a_link_at_its_temporary_name() -> Result<(), Box<dyn Error>> {
    let dir = folder("planted")?;
    let (other, path) = (dir.join("other.txt"), dir.join("mine.json"));
    std::fs::write(&other, "keep me")?;
    std::os::unix::fs::symlink(&other, path.with_extension("json.tmp"))?;
    super::write(&path, "{}")?;
    assert_eq!(std::fs::read_to_string(&other)?, "keep me");
    assert_eq!(std::fs::read_to_string(&path)?, "{}");
    assert!(!std::fs::symlink_metadata(&path)?.is_symlink());
    Ok(())
}
