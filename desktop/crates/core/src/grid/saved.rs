//! The five saved layouts, as data (record 2.7.4), written for chat on the right, and the
//! layouts a person keeps of their own.

use serde::{Deserialize, Serialize};

use crate::grid::layout::{Area, Axis, ChatSide, GridError, Layout, Rect, Size, Track, area};
use crate::grid::leave::close_up;
use crate::grid::place::place;
use crate::ids::PaneId;
use crate::pane::PaneKind;

/// The name a saved layout goes by in a message, a menu and a file, such as `just-chat`.
pub type Slug = &'static str;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Saved {
    pub slug: Slug,
    pub name: &'static str,
    pub cols: &'static [Track],
    pub rows: &'static [Track],
    pub areas: &'static [(PaneKind, Area)],
}

impl Saved {
    /// This layout for one window; `pane` gives the pane that shows each kind.
    pub fn open(&self, mut pane: impl FnMut(PaneKind) -> PaneId) -> Layout {
        self.seat(self.areas.iter().map(|(kind, _)| pane(*kind)))
    }

    /// This layout with its slots given, in the order it lists them, to `panes`.
    pub fn seat(&self, panes: impl IntoIterator<Item = PaneId>) -> Layout {
        self.shape().seat(panes)
    }

    pub fn shape(&self) -> Shape {
        Shape {
            cols: self.cols.to_vec(),
            rows: self.rows.to_vec(),
            areas: self.areas.to_vec(),
        }
    }
}

/// A layout by pane kind: its tracks, and the kind each slot shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub cols: Vec<Track>,
    pub rows: Vec<Track>,
    #[serde(with = "crate::grid::layout::store::kinds")]
    pub areas: Vec<(PaneKind, Area)>,
}

impl Shape {
    /// The slots given, in the order the shape lists them, to `panes`.
    pub fn seat(&self, panes: impl IntoIterator<Item = PaneId>) -> Layout {
        let slots = self.areas.iter().map(|(_, at)| *at);
        Layout {
            cols: self.cols.clone(),
            rows: self.rows.clone(),
            areas: panes.into_iter().zip(slots).collect(),
            away: Vec::new(),
        }
    }

    /// A window's layout as it is drawn, closed up round the panes that are out.
    pub fn of(layout: &Layout, kind: impl Fn(PaneId) -> Option<PaneKind>) -> Option<Shape> {
        let shown = close_up(layout).ok()?;
        let areas = shown
            .areas
            .iter()
            .map(|(pane, at)| Some((kind(*pane)?, *at)));
        Some(Shape {
            cols: shown.tracks(layout, Axis::Cols),
            rows: shown.tracks(layout, Axis::Rows),
            areas: areas.collect::<Option<_>>()?,
        })
    }

    /// The shape as a picture `width` across: each kind where `place` puts it in a grid of
    /// `room`, scaled down whole.
    pub fn picture(&self, room: Size, side: ChatSide, width: f32) -> Vec<(PaneKind, Rect)> {
        let scale = if room.w > 0.0 { width / room.w } else { 0.0 };
        let layout = self.seat((1..).map(PaneId));
        let kind = |pane: PaneId| {
            let at = usize::try_from(pane.0.checked_sub(1)?).ok()?;
            self.areas.get(at).map(|(kind, _)| *kind)
        };
        let placed = place(&layout, room, side).into_iter();
        let scaled = placed.filter_map(|(pane, r)| {
            let (x, y, w, h) = (r.x * scale, r.y * scale, r.w * scale, r.h * scale);
            Some((kind(pane)?, Rect { x, y, w, h }))
        });
        scaled.collect()
    }

    pub fn check(&self) -> Result<(), GridError> {
        self.seat((1..).map(PaneId)).check()
    }
}

/// The number a kept layout goes by: its command's, and from 6 to 9 its ⌥⌘ key's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OwnId(pub u8);

/// The first kept layout's number: the saved five hold ⌥⌘1 to ⌥⌘5.
pub const FIRST_OWN: u8 = 6;
/// Numbers up to this one carry a ⌥⌘ key; later ones have none.
pub const LAST_KEYED: u8 = 9;
/// The most layouts a person keeps, and the longest name one takes, in characters.
pub const MAX_OWN: u8 = 8;
pub const MAX_NAME: usize = 40;

/// A layout a person arranged and named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Own {
    pub id: OwnId,
    pub name: String,
    pub shape: Shape,
}

impl Own {
    /// Whether a file's layout can be kept: a number in range, a name, a shape that tiles.
    pub(crate) fn fits(&self) -> bool {
        let numbered = (FIRST_OWN..FIRST_OWN.saturating_add(MAX_OWN)).contains(&self.id.0);
        numbered && named(&self.name).is_some() && self.shape.check().is_ok()
    }
}

/// A layout's name as typed, trimmed; none when nothing is left or it runs too long.
pub fn named(typed: &str) -> Option<String> {
    let name = typed.trim();
    let fits = !name.is_empty() && name.chars().count() <= MAX_NAME;
    fits.then(|| name.to_owned())
}

use PaneKind::{Changes, Chat, Code, Files, Remote, Report, Terminal, Web};
use Track::{Fr, Px};

pub const EVERYTHING: Saved = Saved {
    slug: "everything",
    name: "Everything",
    cols: &[Px(208.0), Fr(1.0), Px(286.0), Px(270.0)],
    rows: &[Px(226.0), Px(190.0), Fr(1.0)],
    areas: &[
        (Files, area(1, 1, 4, 2)),
        (Changes, area(1, 2, 3, 3)),
        (Terminal, area(3, 2, 4, 3)),
        (Web, area(1, 3, 2, 4)),
        (Remote, area(2, 3, 3, 4)),
        (Report, area(3, 3, 4, 4)),
        (Code, area(1, 4, 2, 5)),
        (Chat, area(2, 4, 4, 5)),
    ],
};

pub const REVIEW: Saved = Saved {
    slug: "review",
    name: "Review",
    cols: &[Px(220.0), Fr(1.0), Px(300.0)],
    rows: &[Fr(1.0), Px(150.0)],
    areas: &[
        (Files, area(1, 1, 3, 2)),
        (Changes, area(1, 2, 2, 3)),
        (Terminal, area(2, 2, 3, 3)),
        (Chat, area(1, 3, 3, 4)),
    ],
};

pub const SCREENS: Saved = Saved {
    slug: "screens",
    name: "Screens",
    cols: &[Fr(1.0), Px(340.0), Px(290.0)],
    rows: &[Fr(1.0), Fr(1.0)],
    areas: &[
        (Web, area(1, 1, 3, 2)),
        (Remote, area(1, 2, 2, 3)),
        (Report, area(2, 2, 3, 3)),
        (Chat, area(1, 3, 3, 4)),
    ],
};

pub const CODE: Saved = Saved {
    slug: "code",
    name: "Code",
    cols: &[Px(220.0), Fr(1.0), Fr(1.0)],
    rows: &[Fr(1.0), Px(170.0)],
    areas: &[
        (Files, area(1, 1, 3, 2)),
        (Code, area(1, 2, 2, 3)),
        (Changes, area(1, 3, 2, 4)),
        (Terminal, area(2, 2, 3, 4)),
    ],
};

pub const JUST_CHAT: Saved = Saved {
    slug: "just-chat",
    name: "Just chat",
    cols: &[Fr(1.0)],
    rows: &[Fr(1.0)],
    areas: &[(Chat, area(1, 1, 2, 2))],
};

pub const SAVED: [Saved; 5] = [EVERYTHING, REVIEW, SCREENS, CODE, JUST_CHAT];

/// A window as it first opens and as a new thread opens it: the chat in the grid, no other
/// pane until one is asked for. It is no saved layout a person picks.
pub const BLANK: Saved = Saved {
    slug: "chat",
    name: "Chat",
    ..JUST_CHAT
};

/// The layout named `slug`: a saved one, or the blank window.
pub fn find(slug: &str) -> Option<Saved> {
    SAVED
        .into_iter()
        .chain([BLANK])
        .find(|saved| saved.slug == slug)
}

#[cfg(test)]
mod own;
#[cfg(test)]
mod tests {
    use crate::grid::layout::{ChatSide, Rect};
    use crate::grid::place::place;
    use crate::grid::place::probe::{Check, FRAME_1, find, open, pane, rect, tiled};

    use super::{
        CODE, Changes, Chat, Code, EVERYTHING, Files, JUST_CHAT, PaneKind, REVIEW, Remote, Report,
        SAVED, SCREENS, Saved, Terminal, Web,
    };

    /// Record 2.7.4, column "Pixel result": the frame 1 grid of the 1200 by 750 window.
    fn record() -> [(Saved, Vec<(PaneKind, Rect)>); 5] {
        [
            (
                EVERYTHING,
                vec![
                    (Files, rect(0.0, 0.0, 208.0, 676.0)),
                    (Changes, rect(208.0, 0.0, 270.0, 416.0)),
                    (Terminal, rect(208.0, 416.0, 270.0, 260.0)),
                    (Web, rect(478.0, 0.0, 286.0, 226.0)),
                    (Remote, rect(478.0, 226.0, 286.0, 190.0)),
                    (Report, rect(478.0, 416.0, 286.0, 260.0)),
                    (Code, rect(764.0, 0.0, 270.0, 226.0)),
                    (Chat, rect(764.0, 226.0, 270.0, 450.0)),
                ],
            ),
            (
                REVIEW,
                vec![
                    (Files, rect(0.0, 0.0, 220.0, 676.0)),
                    (Changes, rect(220.0, 0.0, 514.0, 526.0)),
                    (Terminal, rect(220.0, 526.0, 514.0, 150.0)),
                    (Chat, rect(734.0, 0.0, 300.0, 676.0)),
                ],
            ),
            (
                SCREENS,
                vec![
                    (Web, rect(0.0, 0.0, 404.0, 676.0)),
                    (Remote, rect(404.0, 0.0, 340.0, 338.0)),
                    (Report, rect(404.0, 338.0, 340.0, 338.0)),
                    (Chat, rect(744.0, 0.0, 290.0, 676.0)),
                ],
            ),
            (
                CODE,
                vec![
                    (Files, rect(0.0, 0.0, 220.0, 676.0)),
                    (Code, rect(220.0, 0.0, 407.0, 506.0)),
                    (Changes, rect(627.0, 0.0, 407.0, 506.0)),
                    (Terminal, rect(220.0, 506.0, 814.0, 170.0)),
                ],
            ),
            (JUST_CHAT, vec![(Chat, rect(0.0, 0.0, 1034.0, 676.0))]),
        ]
    }

    /// The five layouts a user picks from, each pane where the record measures it and the
    /// panes it hides given no rectangle.
    #[test]
    fn the_saved_layouts_place_panes_as_the_record_measures_them() -> Check {
        let names = SAVED.map(|saved| saved.name);
        assert_eq!(
            names,
            ["Everything", "Review", "Screens", "Code", "Just chat"]
        );
        for ((saved, want), listed) in record().into_iter().zip(SAVED) {
            assert_eq!(saved, listed, "the list is in the record's order");
            let placed = place(&open(&saved), FRAME_1, ChatSide::Right);
            assert_eq!(placed.len(), want.len(), "{}: panes shown", saved.name);
            tiled(&placed, FRAME_1)?;
            for (kind, rect) in want {
                assert_eq!(find(&placed, pane(kind))?, rect, "{} {kind:?}", saved.name);
            }
        }
        Ok(())
    }

    /// Record 2.7.6: with chat on the left the whole of Everything mirrors, open file over
    /// chat on the far left and Files on the far right.
    #[test]
    fn chat_on_the_left_mirrors_the_whole_grid() -> Check {
        let placed = place(&open(&EVERYTHING), FRAME_1, ChatSide::Left);
        tiled(&placed, FRAME_1)?;
        let want = [
            (Code, rect(0.0, 0.0, 270.0, 226.0)),
            (Chat, rect(0.0, 226.0, 270.0, 450.0)),
            (Web, rect(270.0, 0.0, 286.0, 226.0)),
            (Remote, rect(270.0, 226.0, 286.0, 190.0)),
            (Report, rect(270.0, 416.0, 286.0, 260.0)),
            (Changes, rect(556.0, 0.0, 270.0, 416.0)),
            (Terminal, rect(556.0, 416.0, 270.0, 260.0)),
            (Files, rect(826.0, 0.0, 208.0, 676.0)),
        ];
        for (kind, rect) in want {
            assert_eq!(find(&placed, pane(kind))?, rect, "{kind:?}");
        }
        Ok(())
    }
}
