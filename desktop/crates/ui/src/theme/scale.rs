//! The scale beyond the token table: spacing, weights, radii and a few part colours (record
//! 1.3, 1.4, 1.5), and Yi Dark's file read into tokens, falling back by name.

use yd_core::row::ShapeError;
use yd_core::tokens::{Rgba, ThemeFile, ThemeValue, TokenError, Tokens, rgb, rgba};

use crate::theme::look::{black, white};

/// The spacing ladder: nothing in between (record 1.4.1).
pub const SPACE_2: f32 = 2.0;
pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;

/// Weights in use (record 1.3.2).
pub const REGULAR: f32 = 400.0;
pub const MEDIUM: f32 = 500.0;
pub const SEMIBOLD: f32 = 600.0;
pub const HEAVY: f32 = 650.0;
pub const BOLD: f32 = 700.0;

/// A size a theme may set by name; the record's value otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Num(pub &'static str, pub f32);

impl Num {
    pub fn of(self, tokens: &Tokens) -> f32 {
        tokens.number_or(self.0, self.1)
    }
}

/// A colour a theme may set by name; the record's value otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ink(pub &'static str, pub Rgba);

impl Ink {
    pub fn of(self, tokens: &Tokens) -> Rgba {
        tokens.colour_or(self.0, self.1)
    }
}

pub const RADIUS_CARET: Num = Num("radius.caret", 1.0);
pub const RADIUS_TRACK: Num = Num("radius.track", 2.0);
pub const RADIUS_TRACE: Num = Num("radius.trace", 2.5);
pub const RADIUS_BADGE: Num = Num("radius.badge", 3.0);
pub const RADIUS_HINT: Num = Num("radius.hint", 4.0);
pub const RADIUS_CHIP: Num = Num("radius.chip", 5.0);
pub const RADIUS_BUTTON: Num = Num("radius.button", 6.0);
pub const RADIUS_CARD: Num = Num("radius.card", 7.0);
pub const RADIUS_MESSAGE: Num = Num("radius.message", 8.0);
pub const RADIUS_MENU: Num = Num("radius.menu", 9.0);
pub const RADIUS_WINDOW: Num = Num("radius.window", 10.0);
pub const RADIUS_SUGGESTION: Num = Num("radius.suggestion", 11.0);
pub const RADIUS_COMPOSER: Num = Num("radius.composer", 14.0);
/// How strongly the wash paints behind the panes: 0 shows the desktop through the glass.
pub const WASH: Num = Num("wash", 1.0);

pub const ON_PRIMARY: Ink = Ink("text.on_primary", rgb(0x11, 0x12, 0x14));
pub const ON_ACCENT: Ink = Ink("text.on_accent", rgb(0x0b, 0x0d, 0x12));
pub const ON_OK: Ink = Ink("text.on_ok", rgb(0x08, 0x13, 0x0c));
pub const ON_MODEL: Ink = Ink("text.on_model", rgb(0xff, 0xff, 0xff));
pub const ON_CONTEXT: Ink = Ink("context.on", rgb(0x0a, 0x0e, 0x17));
pub const CONTEXT_TAG: Ink = Ink("context.tag", rgb(0x0c, 0x10, 0x18));
pub const CONTEXT_PREVIEW: Ink = Ink("context.preview", white(1.0));
pub const PILL_FIXING: Ink = Ink("pill.fixing", rgb(0xa9, 0xc4, 0xff));

pub const CARD: Ink = Ink("fill.card", rgba(22, 24, 30, 0.94));
pub const MENU: Ink = Ink("fill.menu", rgba(28, 30, 37, 0.98));
pub const POPOVER: Ink = Ink("fill.popover", rgb(24, 26, 33));
pub const TIP: Ink = Ink("fill.tip", rgb(30, 32, 39));
pub const HELD: Ink = Ink("fill.pane_held", rgba(14, 16, 20, 0.74));
pub const PAPER: Ink = Ink("fill.paper", rgb(0xff, 0xff, 0xff));
pub const HOVER: Ink = Ink("fill.hover", white(0.1));
pub const HOVER_QUIET: Ink = Ink("fill.hover_quiet", white(0.04));
pub const PRESS: Ink = Ink("fill.press", black(0.2));
pub const SOFT: Ink = Ink("fill.soft", white(0.055));
pub const COMPOSER: Ink = Ink("fill.composer", white(0.07));
pub const COMPOSER_PANE: Ink = Ink("fill.composer_pane", white(0.043));
pub const CHIP_QUIET: Ink = Ink("fill.chip_quiet", white(0.07));
pub const STAT: Ink = Ink("fill.stat", white(0.06));
pub const DETAIL: Ink = Ink("fill.detail", white(0.03));
pub const FILE_HEAD: Ink = Ink("fill.file_head", white(0.04));
pub const WELL: Ink = Ink("fill.well", black(0.3));
pub const FIX: Ink = Ink("fill.fix", black(0.28));
pub const STEP: Ink = Ink("fill.step", black(0.26));
pub const NOTE: Ink = Ink("fill.note", black(0.25));
pub const CONSOLE: Ink = Ink("fill.console", black(0.22));
pub const DOT_HOLLOW: Ink = Ink("dot.hollow", black(0.35));
pub const DOT_UNCOVERED: Ink = Ink("dot.uncovered", white(0.14));

pub const LINE_BAR: Ink = Ink("line.bar", white(0.043));
pub const LINE_DIVIDER: Ink = Ink("line.divider", white(0.05));
pub const LINE_CARD: Ink = Ink("line.card", white(0.06));
pub const LINE_SOFT: Ink = Ink("line.soft", white(0.08));
pub const LINE_MENU: Ink = Ink("line.menu", white(0.1));
pub const LINE_SWATCH: Ink = Ink("line.swatch", white(0.2));
pub const LINE_QUIET: Ink = Ink("line.quiet", white(0.25));
pub const LINE_BRANCH: Ink = Ink("line.branch", white(0.28));
pub const LINE_TREE: Ink = Ink("line.tree", white(0.3));
pub const LINE_PICKED: Ink = Ink("line.picked", white(0.4));
pub const TARGET_RING: Ink = Ink("line.drop_target", rgba(143, 180, 255, 0.85));

pub const RING_MENU: Ink = Ink("ring.menu", white(0.12));
pub const RING_CARD: Ink = Ink("ring.card", white(0.14));
pub const RING_SWATCH: Ink = Ink("ring.swatch", white(0.35));
pub const SHADOW_MENU: Ink = Ink("shadow.menu", black(0.8));
pub const SHADOW_HELD: Ink = Ink("shadow.held", black(0.9));
pub const SHADOW_NEAR: Ink = Ink("shadow.near", black(0.5));
pub const SHADOW_TIP: Ink = Ink("shadow.tip", black(0.3));
pub const SHADOW_BUTTON: Ink = Ink("shadow.button", black(0.28));

pub const KNOB: Ink = Ink("toggle.knob", rgb(0xff, 0xff, 0xff));
pub const TOGGLE_OFF: Ink = Ink("toggle.off", white(0.16));
pub const SEGMENT_ON: Ink = Ink("segment.on", white(0.17));
pub const TAB_BAR: Ink = Ink("tab.bar", white(0.035));
pub const TAB_ON: Ink = Ink("tab.on", white(0.13));
pub const TAB_AWAY: Ink = Ink("tab.away", white(0.07));
pub const SLIDER_TRACK: Ink = Ink("slider.track", white(0.14));
pub const METER_TRACK: Ink = Ink("meter.track", white(0.13));
pub const BAR_TRACK: Ink = Ink("bar.track", white(0.07));
pub const BAR_QUIET: Ink = Ink("bar.quiet", white(0.3));
pub const TAG_FILL: Ink = Ink("tag.fill", white(0.09));
pub const BADGE_FILL: Ink = Ink("badge.fill", white(0.1));
pub const FOCUS_RING: Ink = Ink("focus.ring", rgba(220, 223, 230, 0.8));
pub const SELECTION: Ink = Ink("selection.text", rgba(143, 180, 255, 0.3));

pub const BREAKPOINT: Ink = Ink("code.breakpoint", rgb(0xff, 0x5f, 0x57));
pub const CODE_VALUE: Ink = Ink("code.value", rgb(0x8f, 0xa3, 0xc8));
pub const CODE_MATCH: Ink = Ink("code.match", rgb(120, 210, 198));
pub const CODE_INLAY: Ink = Ink("code.inlay", white(0.08));
pub const TERMINAL_BACKGROUND: Ink = Ink("terminal.background", black(0.2));
pub const TERMINAL_CARET_INK: Ink = Ink("terminal.caret_ink", black(1.0));
pub const ON_NOTE: Ink = Ink("text.on_note", rgb(0x16, 0x13, 0x0a));
pub const HANDLE: Ink = Ink("fill.handle", white(1.0));
pub const GUIDE: Ink = Ink("line.guide", rgb(0xff, 0x4f, 0x8b));

/// The mockup's desktop behind thin glass: a dark base lit from the top left, four muted hues
/// near the corners, darker edges and one faint glow.
pub const WASH_BASE: Ink = Ink("wash.base", rgb(0x14, 0x16, 0x1d));
pub const WASH_FROM: Ink = Ink("wash.from", rgb(0x16, 0x1a, 0x26));
pub const WASH_MIDDLE: Ink = Ink("wash.middle", rgb(0x14, 0x16, 0x1e));
pub const WASH_TO: Ink = Ink("wash.to", rgb(0x18, 0x18, 0x20));
pub const WASH_BLUE: Ink = Ink("wash.blue", rgb(0x1f, 0x27, 0x39));
pub const WASH_TEAL: Ink = Ink("wash.teal", rgb(0x1a, 0x2f, 0x33));
pub const WASH_VIOLET: Ink = Ink("wash.violet", rgb(0x2a, 0x23, 0x37));
pub const WASH_WINE: Ink = Ink("wash.wine", rgb(0x33, 0x23, 0x2b));
pub const WASH_EDGE: Ink = Ink("wash.edge", rgba(6, 7, 12, 0.5));
pub const WASH_GLOW: Ink = Ink("wash.glow", rgba(240, 138, 210, 0.09));

/// A named colour as the theme editor shows it: the value, its role and what it is used for.
pub type Part = (Ink, &'static str, &'static str);

/// Every named colour, under the editor group it shows in.
pub const INK_GROUPS: &[(&str, &[Part])] = &[
    (
        "Colour roles",
        &[(PILL_FIXING, "Fixing pill", "a finding being fixed")],
    ),
    (
        "Text",
        &[
            (ON_PRIMARY, "On primary", "words on a primary button"),
            (ON_ACCENT, "On accent", "words on an accent button"),
            (ON_OK, "On running", "words on a running button"),
            (ON_MODEL, "On model", "a model tag's words"),
            (ON_CONTEXT, "On context", "a context box's number"),
            (CONTEXT_TAG, "Context tag", "a box's name tag"),
            (CONTEXT_PREVIEW, "Preview", "a previewed box's frame"),
            (ON_NOTE, "On note", "a mark-up note's words"),
        ],
    ),
    (
        "Surfaces",
        &[
            (CARD, "Card", "hover cards and tooltips"),
            (MENU, "Menu", "menus and the pane picker"),
            (POPOVER, "Popover", "a step's pop-up card"),
            (TIP, "Tip", "a hover chip in chat"),
            (HELD, "Held pane", "a pane being moved"),
            (PAPER, "Paper", "a report's page"),
            (HOVER, "Hover", "under the pointer"),
            (HOVER_QUIET, "Quiet hover", "sidebar rows"),
            (PRESS, "Press", "a control held down"),
            (SOFT, "Soft card", "asks and New chat"),
            (COMPOSER, "Composer", "the reading composer"),
            (COMPOSER_PANE, "Pane composer", "the composer in a pane"),
            (CHIP_QUIET, "Quiet chip", "a script's chips"),
            (STAT, "Stat", "a report's figures"),
            (DETAIL, "Detail", "a finding's detail"),
            (FILE_HEAD, "File head", "a changed file's row"),
            (WELL, "Well", "commands and code in a row"),
            (FIX, "Fix", "a finding's proposed fix"),
            (STEP, "Step", "a step card, the context tray"),
            (NOTE, "Note", "notes beside lines"),
            (CONSOLE, "Console", "the debugger's console"),
            (DOT_HOLLOW, "Hollow dot", "a thread with nothing open"),
            (DOT_UNCOVERED, "Uncovered", "a check with no test"),
            (HANDLE, "Handle", "a design box's handles"),
        ],
    ),
    (
        "Lines",
        &[
            (LINE_BAR, "Bar edge", "top, status and side bars"),
            (LINE_DIVIDER, "Divider", "between rows of a card"),
            (LINE_CARD, "Card line", "inside a card"),
            (LINE_SOFT, "Soft edge", "tooltips, soft frames"),
            (LINE_MENU, "Menu line", "menu rules, frames"),
            (LINE_SWATCH, "Swatch edge", "a colour setting's swatch"),
            (LINE_QUIET, "Quiet mark", "quotes, the turn map"),
            (LINE_BRANCH, "Branch", "the sidebar's tree"),
            (LINE_TREE, "Tree", "a list's tree"),
            (LINE_PICKED, "Picked bar", "beside the picked row"),
            (TARGET_RING, "Drop target", "where a pane would land"),
            (GUIDE, "Guide", "design mode's measured gaps"),
        ],
    ),
    (
        "Shadows",
        &[
            (RING_MENU, "Menu ring", "round menus"),
            (RING_CARD, "Card ring", "round cards, a held pane"),
            (RING_SWATCH, "Swatch ring", "round a swatch"),
            (SHADOW_MENU, "Menu shadow", "under menus and cards"),
            (SHADOW_HELD, "Held shadow", "under a held pane"),
            (SHADOW_NEAR, "Near shadow", "close under a lifted thing"),
            (SHADOW_TIP, "Tip shadow", "under a hover chip"),
            (SHADOW_BUTTON, "Button shadow", "under a button"),
        ],
    ),
    (
        "Controls",
        &[
            (KNOB, "Knob", "a toggle's knob"),
            (TOGGLE_OFF, "Toggle off", "a toggle that is off"),
            (SEGMENT_ON, "Segment", "the picked segment"),
            (TAB_BAR, "Tab bar", "behind a pane's tabs"),
            (TAB_ON, "Tab", "the tab in use"),
            (TAB_AWAY, "Tab away", "a tab of another pane"),
            (SLIDER_TRACK, "Slider", "a slider's track"),
            (METER_TRACK, "Meter", "a meter's track"),
            (BAR_TRACK, "Bar track", "behind a list's bars"),
            (BAR_QUIET, "Quiet bar", "a list bar with no role"),
            (TAG_FILL, "Tag", "a small tag"),
            (BADGE_FILL, "Badge", "a badge or pill with no role"),
            (FOCUS_RING, "Focus ring", "the control the keyboard is on"),
            (SELECTION, "Selection", "selected text"),
        ],
    ),
    (
        "Code",
        &[
            (BREAKPOINT, "Breakpoint", "a line that stops"),
            (CODE_VALUE, "Value", "a value shown in a line"),
            (CODE_MATCH, "Match", "a found reference"),
            (CODE_INLAY, "Inlay", "behind a value in a line"),
        ],
    ),
    (
        "Wash",
        &[
            (WASH_BASE, "Base", "under the whole wash"),
            (WASH_FROM, "Light", "the lit top left"),
            (WASH_MIDDLE, "Middle", "the middle of the light"),
            (WASH_TO, "Shade", "the far bottom right"),
            (WASH_BLUE, "Blue", "the top left hue"),
            (WASH_TEAL, "Teal", "the top right hue"),
            (WASH_VIOLET, "Violet", "the bottom right hue"),
            (WASH_WINE, "Wine", "the bottom left hue"),
            (WASH_EDGE, "Edges", "the darker edges"),
            (WASH_GLOW, "Glow", "a faint glow low on the right"),
        ],
    ),
    (
        "Terminal",
        &[
            (TERMINAL_BACKGROUND, "Background", "behind a terminal"),
            (
                TERMINAL_CARET_INK,
                "Caret ink",
                "the letter under the caret",
            ),
        ],
    ),
];

/// Every named size above, for the theme loader to check a file against.
pub const NUMS: &[Num] = &[
    RADIUS_CARET,
    RADIUS_TRACK,
    RADIUS_TRACE,
    RADIUS_BADGE,
    RADIUS_HINT,
    RADIUS_CHIP,
    RADIUS_BUTTON,
    RADIUS_CARD,
    RADIUS_MESSAGE,
    RADIUS_MENU,
    RADIUS_WINDOW,
    RADIUS_SUGGESTION,
    RADIUS_COMPOSER,
    WASH,
];

/// Every named colour above, for the theme loader to check a file against.
pub fn inks() -> impl Iterator<Item = Ink> {
    INK_GROUPS
        .iter()
        .flat_map(|(_, parts)| parts.iter().map(|(ink, _, _)| *ink))
}

/// The default theme as a file, the shape every theme takes.
pub const YI_DARK: &str = include_str!("yi-dark.json");

/// A part of a theme that was not taken; Yi Dark's value stands in for it.
#[derive(Debug)]
pub enum Fallback {
    /// The file is not a theme at all: every value is Yi Dark's.
    Unreadable(ShapeError),
    /// One value has the wrong kind for its name.
    Refused { name: String, why: TokenError },
}

/// A theme's tokens and what it fell back to.
#[derive(Debug)]
pub struct Loaded {
    pub tokens: Tokens,
    pub fell_back: Vec<Fallback>,
}

/// Whether `value` has the kind a named size or colour takes; any other name takes anything.
pub fn check(name: &str, value: &ThemeValue) -> Result<(), TokenError> {
    if NUMS.iter().any(|num| num.0 == name) {
        matches!(value, ThemeValue::Number(n) if n.is_finite())
            .then_some(())
            .ok_or(TokenError::NotANumber)
    } else if inks().any(|ink| ink.0 == name) {
        matches!(value, ThemeValue::Text(text) if Rgba::parse(text).is_some())
            .then_some(())
            .ok_or(TokenError::NotAColour)
    } else {
        Ok(())
    }
}

/// Reads a theme. A bad value keeps Yi Dark's and is named in `fell_back`.
pub fn load(text: &str) -> Loaded {
    let file = match ThemeFile::parse(text) {
        Ok(file) => file,
        Err(cause) => {
            return Loaded {
                tokens: Tokens::default(),
                fell_back: vec![Fallback::Unreadable(cause)],
            };
        }
    };
    let (tokens, refused) = Tokens::themed(&file);
    let mut fell_back: Vec<Fallback> = refused
        .into_iter()
        .map(|(name, why)| Fallback::Refused { name, why })
        .collect();
    // Named extras land in `extra` whatever they hold; a wrong kind is still a fallback.
    for (name, value) in &file.values {
        if let Err(why) = check(name, value) {
            fell_back.push(Fallback::Refused {
                name: name.clone(),
                why,
            });
        }
    }
    Loaded { tokens, fell_back }
}
