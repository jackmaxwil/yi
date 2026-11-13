use std::collections::BTreeMap;
use std::fmt;

/* A palette maps a token kind to a colour name.
   This comment spans three lines, so a read that starts on line 3
   begins inside it. */
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Keyword,
    Str,
    Number,
    Comment,
}

pub struct Palette {
    colours: BTreeMap<Kind, &'static str>,
}

impl Palette {
    pub fn new() -> Self {
        let mut colours = BTreeMap::new();
        colours.insert(Kind::Keyword, "magenta");
        colours.insert(Kind::Str, "green");
        colours.insert(Kind::Number, "yellow");
        colours.insert(Kind::Comment, "grey");
        Self { colours }
    }

    pub fn colour(&self, kind: Kind) -> &'static str {
        self.colours.get(&kind).copied().unwrap_or("white")
    }

    pub fn count(&self) -> usize {
        self.colours.len()
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Palette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (kind, colour) in &self.colours {
            writeln!(f, "{kind:?} = \"{colour}\"")?;
        }
        Ok(())
    }
}

fn main() {
    let palette = Palette::default();
    let total = palette.count() * 2 + 1;
    println!("{palette}");
    println!("total: {total}, keyword: {}", palette.colour(Kind::Keyword));
}
