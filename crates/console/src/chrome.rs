//! Controls drawn as kitty images: the settings gear, each pane's close button and the hint
//! row's keycaps, rastered at the terminal's own cell pixels.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::{ColorTier, Theme};

use crate::app::App;
use crate::keys;
use crate::model::SidebarMode;
use crate::render::{GEAR, Hits, PaneView};

const GEAR_COLS: u16 = 4;
const GEAR_ROWS: u16 = 3;

/// Each hint is a keycap on a dock: the bodies are cell backgrounds so the chord stays real text,
/// and only the rounded ends, cells with no text in them, are images.
pub(crate) fn render_hint_buttons(app: &mut App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    use keys::Action as A;
    let hues = [
        (A::Navigator, theme.accent),
        (A::NewSession, theme.success),
        (A::ToggleSidebar, theme.cyan),
        (A::ToggleNotebook, theme.orange),
        (A::ToggleDiff, theme.error),
        (A::Keys, theme.magenta),
    ];
    let width_of = |text: &str| Line::raw(text).width();
    let mut fit = usize::from(area.width);
    let items = loop {
        let items = keys::hint_items(false, app.cmd_hints, fit);
        let need: usize = items
            .iter()
            .map(|(text, _)| width_of(text) + 5)
            .sum::<usize>()
            + 1;
        if need <= usize::from(area.width) || fit == 0 {
            break items;
        }
        fit -= 1;
    };
    let dark = match theme.tier {
        ColorTier::TrueColor => Color::Rgb(0x1b, 0x1d, 0x2b),
        _ => Color::Black,
    };
    let base = theme
        .user_bg
        .map_or_else(Style::default, |dock| Style::default().bg(dock));
    let buffer = frame.buffer_mut();
    let mut put = |x: u16, text: &str, style: Style| {
        let room = usize::from(area.right().saturating_sub(x));
        buffer.set_stringn(x, area.y, text, room, style).0
    };
    let mut caps: Vec<(u16, Color, bool, Option<Color>)> = Vec::new();
    caps.extend(theme.user_bg.map(|dock| (area.x, dock, true, None)));
    let mut x = area.x.saturating_add(1);
    let mut buttons = Vec::new();
    let count = items.len();
    for (index, (text, action)) in items.into_iter().enumerate() {
        let (Some((chord, label)), Some(action)) = (text.split_once(' '), action) else {
            continue;
        };
        let width = u16::try_from(width_of(&text) + 4).unwrap_or(u16::MAX);
        let button = Rect::new(x, area.y, width, 1).intersection(area);
        let hovered = app
            .pointer
            .is_some_and(|(px, py)| button.contains((px, py).into()));
        let hue = hues
            .iter()
            .find(|(a, _)| *a == action)
            .map_or(theme.accent, |(_, c)| *c);
        let fill = if hovered { lighten(hue) } else { hue };
        x = put(x, " ", base);
        caps.push((x, fill, true, theme.user_bg));
        let key = Style::default()
            .fg(dark)
            .bg(fill)
            .add_modifier(Modifier::BOLD);
        x = put(x.saturating_add(1), chord, key);
        caps.push((x, fill, false, theme.user_bg));
        let text = if hovered { theme.muted } else { theme.dim };
        x = put(x.saturating_add(1), " ", base);
        x = put(x, label, base.fg(text));
        x = put(x, " ", base);
        if index + 1 < count {
            x = put(x, "│", base.fg(theme.dim));
        }
        buttons.push((button, action));
    }
    caps.extend(theme.user_bg.map(|dock| (x, dock, false, None)));
    let (cell_w, h) = cell_pixels();
    for (at, fill, left, under) in caps {
        let Some(cell) = buffer.cell_mut((at, area.y)) else {
            continue;
        };
        cell.set_symbol(" ");
        cell.set_bg(if app.kitty {
            under.unwrap_or(Color::Reset)
        } else {
            fill
        });
        if app.kitty
            && let Some(hits) = app.hits.as_mut()
        {
            hits.avatars.push(crate::avatar::Placement {
                col: at,
                row: area.y,
                cols: 1,
                rows: 1,
                key: format!("cap#{at}#{left}#{cell_w}x{h}#{fill:?}"),
                seed: String::new(),
                accent: rgb(fill),
                state: None,
                art: crate::avatar::Art::Cap {
                    width: cell_w,
                    height: h,
                    left,
                },
            });
        }
    }
    if let Some(hits) = app.hits.as_mut() {
        hits.hint_buttons = buttons;
    }
}

fn lighten(color: Color) -> Color {
    let (r, g, b) = rgb(color);
    let up = |c: u8| c.saturating_add((255 - c) / 4);
    match color {
        Color::Rgb(..) => Color::Rgb(up(r), up(g), up(b)),
        other => other,
    }
}

pub(crate) fn gear_height(sidebar_width: u16, kitty: bool) -> u16 {
    match (sidebar_width, kitty) {
        (0, _) => 0,
        (_, false) => 1,
        (_, true) => GEAR_ROWS,
    }
}

pub(crate) fn render_gear(app: &mut App, frame: &mut Frame<'_>, gear: Rect, theme: &Theme) {
    let key = keys::chord_for(keys::Action::Settings, app.cmd_hints);
    let key = Span::styled(key, theme.dim_style());
    let full = app.state.sidebar == SidebarMode::Full;
    if !app.kitty {
        let label = if full {
            format!(" {GEAR} settings ")
        } else {
            format!(" {GEAR} ")
        };
        let line = Line::from(vec![Span::styled(label, theme.accent_style()), key]);
        frame.render_widget(Paragraph::new(line), gear);
        return;
    }
    let col = gear.x.saturating_add(if full { 1 } else { 2 });
    let (cell_w, cell_h) = cell_pixels();
    let width = cell_w.saturating_mul(usize::from(GEAR_COLS)).clamp(8, 384);
    let height = cell_h.saturating_mul(usize::from(GEAR_ROWS)).clamp(8, 384);
    let accent = rgb(theme.accent);
    if let Some(hits) = app.hits.as_mut() {
        hits.avatars.push(crate::avatar::Placement {
            col,
            row: gear.y,
            cols: GEAR_COLS,
            rows: GEAR_ROWS,
            key: format!("settings-gear#{width}x{height}"),
            seed: String::new(),
            accent,
            state: None,
            art: crate::avatar::Art::Gear { width, height },
        });
    }
    if full {
        let x = col.saturating_add(GEAR_COLS).saturating_add(1);
        let bold = theme.accent_style().add_modifier(Modifier::BOLD);
        let at = Rect::new(
            x,
            gear.y.saturating_add(1),
            gear.right().saturating_sub(x),
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::styled("Settings", bold)),
            at.intersection(gear),
        );
    }
}

pub(crate) fn closer_image(hits: &mut Hits, pane: &PaneView, at: Rect, theme: &Theme) {
    let fill = rgb(if pane.focused {
        theme.accent
    } else {
        theme.dim
    });
    let (cell_w, cell_h) = cell_pixels();
    let (w, h) = (cell_w.saturating_mul(usize::from(at.width)), cell_h);
    hits.avatars.push(crate::avatar::Placement {
        col: at.x,
        row: at.y,
        cols: at.width,
        rows: 1,
        key: format!("closer#{}#{w}x{h}#{fill:?}", pane.id.raw()),
        seed: String::new(),
        accent: fill,
        state: None,
        art: crate::avatar::Art::Closer {
            width: w,
            height: h,
        },
    });
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => yi_tui::orb::kitty::INK_RGB,
    }
}

/// 8×16 when the terminal does not report its pixel size.
pub(crate) fn cell_pixels() -> (usize, usize) {
    match ratatui::crossterm::terminal::window_size() {
        Ok(size) if size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0 => (
            usize::from(size.width / size.columns),
            usize::from(size.height / size.rows),
        ),
        _ => (8, 16),
    }
}
