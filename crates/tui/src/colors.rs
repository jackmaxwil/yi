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
}

impl Theme {
    pub fn new(tier: ColorTier, dark: bool) -> Self {
        // Blend ratios from design U17: user-visible dim text is the fg
        // blended into the bg at 12 % on dark, 4 % on light backgrounds.
        let (muted, dim) = match tier {
            ColorTier::TrueColor if dark => (Color::Rgb(140, 140, 140), Color::Rgb(95, 95, 95)),
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
        }
    }

    pub fn dim_style(&self) -> Style {
        match self.tier {
            ColorTier::Ansi16 => Style::default().add_modifier(Modifier::DIM),
            _ => Style::default().fg(self.dim),
        }
    }

    pub fn muted_style(&self) -> Style {
        match self.tier {
            ColorTier::Ansi16 => Style::default().add_modifier(Modifier::DIM),
            _ => Style::default().fg(self.muted),
        }
    }
}

const ACCENTS: [Color; 6] = [
    Color::Cyan,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::LightRed,
];

/// Stable identity color from a name (OMP `getSessionAccentHex`): every
/// session and subagent gets the same accent every time it renders.
pub fn name_accent(name: &str) -> Color {
    let mut hash = 0_u32;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    let index = (hash as usize) % ACCENTS.len();
    ACCENTS.get(index).copied().unwrap_or(Color::Cyan)
}
