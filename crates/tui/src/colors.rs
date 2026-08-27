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
/// Absent or unparseable defaults to dark (design U17).
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
    /// The ground lifted toward white. codex probes the terminal's background
    /// and drops the tint when it cannot; Yi has no probe, so the tint is offered
    /// only for the truecolor dark theme whose ground it already assumes.
    pub user_bg: Option<Color>,
}

impl Theme {
    pub fn new(tier: ColorTier, dark: bool) -> Self {
        // Truecolor dark is the primary look: TokyoNight Moon values, chosen
        // to sit on translucent dark grounds (Ghostty blur) without banding.
        // Text stays Color::Reset so the terminal's own fg wins.
        if tier == ColorTier::TrueColor && dark {
            return Self {
                tier,
                dark,
                accent: Color::Rgb(0x82, 0xaa, 0xff),
                text: Color::Reset,
                muted: Color::Rgb(0x82, 0x8b, 0xb8),
                dim: Color::Rgb(0x63, 0x6d, 0xa6),
                error: Color::Rgb(0xff, 0x75, 0x7f),
                warning: Color::Rgb(0xff, 0xc7, 0x77),
                success: Color::Rgb(0xc3, 0xe8, 0x8d),
                user_bg: Some(Color::Rgb(0x2f, 0x33, 0x47)),
            };
        }
        let (muted, dim) = match tier {
            ColorTier::TrueColor => (Color::Rgb(120, 120, 120), Color::Rgb(160, 160, 160)),
            _ => (Color::DarkGray, Color::DarkGray),
        };
        Self {
            tier,
            dark,
            accent: Color::Cyan,
            text: Color::Reset,
            muted,
            dim,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            user_bg: None,
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

    pub fn muted_style(&self) -> Style {
        match self.tier {
            ColorTier::Ansi16 => Style::default().add_modifier(Modifier::DIM),
            _ => Style::default().fg(self.muted),
        }
    }
}

const ACCENTS: [Color; 6] = [
    Color::Rgb(0x82, 0xaa, 0xff),
    Color::Rgb(0xc0, 0x99, 0xff),
    Color::Rgb(0x4f, 0xd6, 0xbe),
    Color::Rgb(0xff, 0xc7, 0x77),
    Color::Rgb(0xc3, 0xe8, 0x8d),
    Color::Rgb(0xff, 0x96, 0x6c),
];

/// Stable, so a session or subagent keeps one accent across renders.
pub fn name_accent(name: &str) -> Color {
    let mut hash = 0_u32;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    let index = (hash as usize) % ACCENTS.len();
    ACCENTS.get(index).copied().unwrap_or(Color::Cyan)
}
