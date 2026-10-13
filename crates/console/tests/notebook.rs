use ratatui::text::Line;
use yi_console::model::NbCell;
use yi_console::panes::notebook_lines;
use yi_tui::colors::{ColorTier, Theme};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rows(cells: &[NbCell], width: usize, full: bool) -> Vec<String> {
    let theme = Theme::new(ColorTier::TrueColor, true);
    notebook_lines(cells, width, full, &theme, "⌘W")
        .iter()
        .map(|line: &Line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("x{n}\n")).collect()
}

#[test]
fn every_cell_cap_speaks_one_row_past_its_limit() -> TestResult {
    type Fill = fn(&mut NbCell, String);
    let fields: [(&str, usize, bool, Fill); 5] = [
        ("code", 24, false, |cell, text| cell.code = text),
        ("stdout", 40, false, |cell, text| cell.stdout = text),
        ("stderr", 40, false, |cell, text| cell.stderr = text),
        ("result", 20, false, |cell, text| cell.result = Some(text)),
        ("traceback", 12, true, |cell, text| cell.error = Some(text)),
    ];
    for (noun, cap, tail, fill) in fields {
        let mut cell = NbCell {
            number: 1,
            ..NbCell::default()
        };
        fill(&mut cell, numbered(cap));
        let whole = rows(std::slice::from_ref(&cell), 80, false);
        assert!(
            !whole.iter().any(|row| row.contains("[…]")),
            "{noun} at cap: {whole:#?}"
        );
        assert!(
            whole.iter().any(|row| row.ends_with(&format!("x{cap}"))),
            "{noun}"
        );
        fill(&mut cell, numbered(cap + 1));
        let cut = rows(std::slice::from_ref(&cell), 80, false);
        let unit = if noun == "code" { "lines" } else { "rows" };
        let end = if tail { "last" } else { "first" };
        let said = format!("[…] {noun} {end} {cap} of {} {unit}", cap + 1);
        assert!(
            cut.iter().any(|row| row.contains(&said)),
            "{said:?}: {cut:#?}"
        );
        let gone = if tail {
            "x1".to_owned()
        } else {
            format!("x{}", cap + 1)
        };
        assert!(
            !cut.iter().any(|row| row.ends_with(&gone)),
            "{noun} kept {gone}"
        );
        let all = rows(std::slice::from_ref(&cell), 80, true);
        assert!(
            !all.iter().any(|row| row.contains("[…]")),
            "{noun} in full view"
        );
        assert!(
            all.iter().any(|row| row.ends_with(&gone)),
            "{noun} full lost {gone}"
        );
    }
    Ok(())
}
