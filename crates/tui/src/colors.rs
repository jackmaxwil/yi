use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTier {
    TrueColor,
    Ansi256,
    Ansi16,
}

pub fn detect_tier(colorterm: Option<&str>, term: Option<&str>) -> ColorTier {
    if colorterm.is_some_and(|v| v.contains("truecolor") || v.contains("24bit")) {
        return ColorTier::TrueColor;
    }
    match term {
        Some(t) if t.contains("256color") => ColorTier::Ansi256,
        Some(t) if t.contains("truecolor") => ColorTier::TrueColor,
        _ => ColorTier::Ansi16,
    }
}

/// `COLORFGBG` is `"<fg>;<bg>"`; bg 0-6 or 8 means a dark background.
/// Absent or unparseable defaults to dark (design §17.3).
pub fn detect_dark(colorfgbg: Option<&str>) -> bool {
    let Some(value) = colorfgbg else { return true };
    let Some(bg) = value.rsplit(';').next() else {
        return true;
    };
    match bg.trim().parse::<u8>() {
        Ok(n) => n < 7 || n == 8,
        Err(_) => true,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub tier: ColorTier,
    pub dark: bool,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub dim: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub teal: Color,
    pub orange: Color,
    pub purple: Color,
    pub blue5: Color,
    /// The ground lifted toward white. Yi cannot probe the terminal background,
    /// so the tint is offered only where the theme already assumes its ground.
    pub user_bg: Option<Color>,
}

impl Theme {
    pub fn new(tier: ColorTier, dark: bool) -> Self {
        let pick = |named, indexed| match tier {
            ColorTier::Ansi16 => named,
            _ => Color::Indexed(indexed),
        };
        let (muted, dim) = match tier {
            ColorTier::TrueColor => (Color::Rgb(120, 120, 120), Color::Rgb(160, 160, 160)),
            _ => (Color::DarkGray, Color::DarkGray),
        };
        let base = Self {
            tier,
            dark,
            accent: Color::Cyan,
            text: Color::Reset,
            muted,
            dim,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            magenta: pick(Color::Magenta, 176),
            cyan: pick(Color::Cyan, 117),
            teal: pick(Color::Cyan, 79),
            orange: pick(Color::Yellow, 209),
            purple: pick(Color::Magenta, 219),
            blue5: pick(Color::LightBlue, 123),
            user_bg: None,
        };
        if tier != ColorTier::TrueColor || !dark {
            return base;
        }
        // Truecolor dark is the primary look: TokyoNight Moon values, chosen to sit on
        // translucent grounds without banding. Text stays Color::Reset so the terminal wins.
        Self {
            accent: Color::Rgb(0x82, 0xaa, 0xff),
            muted: Color::Rgb(0x82, 0x8b, 0xb8),
            dim: Color::Rgb(0x63, 0x6d, 0xa6),
            error: Color::Rgb(0xff, 0x75, 0x7f),
            warning: Color::Rgb(0xff, 0xc7, 0x77),
            success: Color::Rgb(0xc3, 0xe8, 0x8d),
            magenta: Color::Rgb(0xc0, 0x99, 0xff),
            cyan: Color::Rgb(0x86, 0xe1, 0xfc),
            teal: Color::Rgb(0x4f, 0xd6, 0xbe),
            orange: Color::Rgb(0xff, 0x96, 0x6c),
            purple: Color::Rgb(0xfc, 0xa7, 0xea),
            blue5: Color::Rgb(0x89, 0xdd, 0xff),
            user_bg: Some(Color::Rgb(0x2f, 0x33, 0x47)),
            ..base
        }
    }

    pub fn dim_style(&self) -> Style {
        match self.tier {
            ColorTier::Ansi16 => Style::default().add_modifier(Modifier::DIM),
            _ => Style::default().fg(self.dim),
        }
    }

    /// Blank spacer rows included, so the band reads as one object.
    pub fn user_style(&self) -> Style {
        match self.user_bg {
            Some(bg) => Style::default().bg(bg),
            None => Style::default(),
        }
    }

    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    /// The dark theme lifts its own ground; every other tier borrows the
    /// terminal's grey rather than inventing a palette.
    pub fn selection_bg(&self) -> Color {
        self.user_bg.unwrap_or(Color::DarkGray)
    }

    /// The row of the session in front, a shade off the ground; the cursor row is louder.
    pub fn active_row_bg(&self) -> Color {
        match self.tier {
            ColorTier::Ansi16 => Color::DarkGray,
            _ if self.dark => Color::Rgb(0x1e, 0x1e, 0x2e),
            _ => Color::Indexed(254),
        }
    }

    pub fn muted_style(&self) -> Style {
        match self.tier {
            ColorTier::Ansi16 => Style::default().add_modifier(Modifier::DIM),
            _ => Style::default().fg(self.muted),
        }
    }
}

/// The four layers a diff row paints: the line-number gutter, the sign column,
/// the content, and the tint carried across the rest of the row.
#[derive(Debug, Clone, Copy)]
pub struct DiffRowStyle {
    pub gutter: Style,
    pub sign: Style,
    pub content: Style,
    pub fill: Option<Color>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    Context,
    Added,
    Removed,
}

impl Theme {
    /// Values ported verbatim: light needs a more saturated gutter to hold a
    /// number on the pastel, and at 16 colours the terminal owns the ground.
    pub fn diff_row(&self, kind: DiffRowKind) -> DiffRowStyle {
        let added = kind == DiffRowKind::Added;
        if kind == DiffRowKind::Context {
            return DiffRowStyle {
                gutter: self.dim_style(),
                sign: Style::default(),
                content: Style::default().fg(self.text),
                fill: None,
            };
        }
        let polarity = if added { self.success } else { self.error };
        match (self.tier, self.dark) {
            (ColorTier::Ansi16, _) => DiffRowStyle {
                gutter: Style::default().add_modifier(Modifier::DIM),
                sign: Style::default().fg(polarity),
                content: Style::default().fg(polarity),
                fill: None,
            },
            (ColorTier::TrueColor, true) => {
                let bg = if added {
                    Color::Rgb(0x21, 0x3A, 0x2B)
                } else {
                    Color::Rgb(0x4A, 0x22, 0x1D)
                };
                DiffRowStyle {
                    gutter: self.dim_style().bg(bg),
                    sign: Style::default().fg(polarity).bg(bg),
                    content: Style::default().fg(polarity).bg(bg),
                    fill: Some(bg),
                }
            }
            (ColorTier::TrueColor, false) => {
                let (bg, gutter_bg) = if added {
                    (Color::Rgb(0xda, 0xfb, 0xe1), Color::Rgb(0xac, 0xee, 0xbb))
                } else {
                    (Color::Rgb(0xff, 0xeb, 0xe9), Color::Rgb(0xff, 0xce, 0xcb))
                };
                DiffRowStyle {
                    gutter: Style::default()
                        .fg(Color::Rgb(0x1f, 0x23, 0x28))
                        .bg(gutter_bg),
                    sign: Style::default().fg(polarity).bg(bg),
                    content: Style::default().bg(bg),
                    fill: Some(bg),
                }
            }
            (ColorTier::Ansi256, true) => {
                let bg = Color::Indexed(if added { 22 } else { 52 });
                DiffRowStyle {
                    gutter: self.dim_style().bg(bg),
                    sign: Style::default().fg(polarity).bg(bg),
                    content: Style::default().fg(polarity).bg(bg),
                    fill: Some(bg),
                }
            }
            (ColorTier::Ansi256, false) => {
                let (bg, gutter_bg) = if added {
                    (Color::Indexed(194), Color::Indexed(157))
                } else {
                    (Color::Indexed(224), Color::Indexed(217))
                };
                DiffRowStyle {
                    gutter: Style::default().fg(Color::Indexed(236)).bg(gutter_bg),
                    sign: Style::default().fg(polarity).bg(bg),
                    content: Style::default().bg(bg),
                    fill: Some(bg),
                }
            }
        }
    }
}

pub const ACCENT_RGB: [(u8, u8, u8); 14] = [
    (0x82, 0xaa, 0xff),
    (0xc0, 0x99, 0xff),
    (0x4f, 0xd6, 0xbe),
    (0xff, 0xc7, 0x77),
    (0xc3, 0xe8, 0x8d),
    (0xff, 0x96, 0x6c),
    (0xfc, 0xa7, 0xea),
    (0x86, 0xe1, 0xfc),
    (0xb4, 0xbd, 0xff),
    (0xe0, 0xaf, 0x68),
    (0x9e, 0xce, 0x6a),
    (0xff, 0x75, 0x7f),
    (0x73, 0xda, 0xca),
    (0xff, 0xd7, 0xa3),
];

pub fn accent(index: usize) -> Color {
    let (r, g, b) = accent_rgb(index);
    Color::Rgb(r, g, b)
}

pub fn accent_rgb(index: usize) -> (u8, u8, u8) {
    ACCENT_RGB
        .get(index % ACCENT_RGB.len())
        .copied()
        .unwrap_or((0x4f, 0xd6, 0xbe))
}

pub fn accent_index(name: &str) -> usize {
    let mut hash = 0_u32;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    (hash as usize) % ACCENT_RGB.len()
}

/// Stable, so a session or subagent keeps one accent across renders.
pub fn name_accent(name: &str) -> Color {
    accent(accent_index(name))
}

/// The same accent as bytes, for a raster that must agree with the text beside it.
pub fn name_accent_rgb(name: &str) -> (u8, u8, u8) {
    accent_rgb(accent_index(name))
}

/// Two-letter tile: the first two alphanumerics, uppercased; `··` for nothing to say.
pub fn name_tile(name: &str) -> String {
    let letters: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    match letters.chars().count() {
        0 => "··".to_owned(),
        1 => format!("{letters}·"),
        _ => letters,
    }
}

/// White initials on the name's accent: the text form of the avatar, drawn everywhere.
pub fn tile_style(name: &str) -> Style {
    tile_style_at(accent_index(name))
}

pub fn tile_style_at(index: usize) -> Style {
    Style::default()
        .fg(Color::Rgb(0x1a, 0x1b, 0x26))
        .bg(accent(index))
        .add_modifier(Modifier::BOLD)
}
