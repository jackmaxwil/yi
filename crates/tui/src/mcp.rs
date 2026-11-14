use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use yi_runtime::mcp_page;
pub use yi_runtime::mcp_page::McpRow;

use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::popup::{BottomView, PopupResult};

enum Outcome {
    Rows(String, Vec<McpRow>),
    Failed(String),
}

type Slot = Arc<Mutex<Option<Outcome>>>;
type Probe = fn(&McpRow) -> Result<Vec<McpRow>, String>;
pub struct McpPage {
    home: Option<PathBuf>,
    rows: Vec<McpRow>,
    selected: usize,
    enabled: bool,
    note: String,
    error: Option<String>,
    busy: Option<(String, Slot)>,
    probe: Option<Probe>,
}

impl crate::app::App {
    pub fn open_mcp(&mut self) {
        self.bottom = Some(crate::app::Bottom::Mcp(Box::new(McpPage::open(
            crate::theme_file::home(),
        ))));
        self.scheduler.request();
    }

    pub(crate) fn poll_mcp(&mut self) {
        if let Some(crate::app::Bottom::Mcp(page)) = &mut self.bottom
            && page.poll()
        {
            self.scheduler.request();
        }
    }
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn mark(row: &McpRow) -> &'static str {
    if !row.enabled {
        return "✕";
    }
    match row.state.as_deref() {
        Some("live") => "●",
        Some("unauthorized" | "expired") => "!",
        _ => "◌",
    }
}

impl McpPage {
    pub fn open(home: Option<PathBuf>) -> Self {
        let enabled = home.as_deref().is_some_and(mcp_page::enabled);
        let mut page = Self::from_rows(home, enabled, Vec::new());
        if page.enabled {
            page.list_servers();
        }
        page
    }

    pub fn from_rows(home: Option<PathBuf>, enabled: bool, rows: Vec<McpRow>) -> Self {
        Self {
            home,
            rows,
            selected: 0,
            enabled,
            note: String::new(),
            error: None,
            busy: None,
            probe: None,
        }
    }

    pub fn paste(&mut self, _text: &str) {}

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    fn reload(&mut self) {
        self.error = None;
        let Some(home) = self.home.as_deref() else {
            self.error = Some("HOME is not set, so the config cannot be found".to_owned());
            return;
        };
        self.enabled = mcp_page::enabled(home);
        self.rows.clear();
        self.selected = 0;
        if self.enabled {
            self.list_servers();
        }
    }

    fn list_servers(&mut self) {
        self.spawn(
            "listing MCP servers".to_owned(),
            || match mcp_page::servers() {
                Ok(rows) => Outcome::Rows("loaded MCP servers".to_owned(), rows),
                Err(error) => Outcome::Failed(error),
            },
        );
    }

    fn selected_row(&self) -> Option<&McpRow> {
        self.rows.get(self.selected)
    }

    fn toggle_enabled(&mut self) {
        let Some(home) = self.home.clone() else {
            return;
        };
        let now = !self.enabled;
        match mcp_page::set_enabled(&home, now) {
            Ok(()) => {
                self.note = format!("mcp.enabled is now {}", if now { "on" } else { "off" });
                self.reload();
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn spawn(&mut self, label: String, job: impl FnOnce() -> Outcome + Send + 'static) {
        let slot: Slot = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&slot);
        std::thread::spawn(move || {
            let outcome = job();
            if let Ok(mut guard) = writer.lock() {
                *guard = Some(outcome);
            }
        });
        self.error = None;
        self.busy = Some((label, slot));
    }

    fn probe(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let name = row.name.clone();
        let probe = self.probe.unwrap_or(mcp_page::probe);
        self.spawn(format!("probing {name}"), move || match probe(&row) {
            Ok(rows) => Outcome::Rows(format!("probed {name}"), rows),
            Err(error) => Outcome::Failed(format!("probe {name}: {error}")),
        });
    }

    fn login(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if !row.is_http() {
            self.error = Some(format!("{} is a stdio server and has no login", row.name));
            return;
        }
        let name = row.name.clone();
        self.spawn(
            format!("logging in to {name} (finish in the browser)"),
            move || {
                if let Err(error) = mcp_page::login(&row) {
                    return Outcome::Failed(format!("login {name}: {error}"));
                }
                match mcp_page::servers() {
                    Ok(rows) => Outcome::Rows(format!("signed in to {name}"), rows),
                    Err(error) => Outcome::Failed(error),
                }
            },
        );
    }

    fn open_config(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let path = row.path.clone();
        match mcp_page::open_config(&path) {
            Ok(()) => self.note = format!("opened {}", path.display()),
            Err(error) => self.error = Some(error),
        }
    }
    pub fn poll(&mut self) -> bool {
        let Some((_, slot)) = &self.busy else {
            return false;
        };
        let Some(outcome) = slot.lock().ok().and_then(|mut guard| guard.take()) else {
            return false;
        };
        self.busy = None;
        match outcome {
            Outcome::Rows(note, rows) => {
                self.note = note;
                self.rows = rows;
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            }
            Outcome::Failed(error) => self.error = Some(error),
        }
        true
    }

    fn header(&self, theme: &Theme) -> Vec<Line<'static>> {
        let (state, style) = if self.enabled {
            ("on", Style::default().fg(theme.success))
        } else {
            ("off", Style::default().fg(theme.warning))
        };
        let mut spans = vec![
            Span::styled(
                " MCP ".to_owned(),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("mcp.enabled {state}"), style),
            Span::styled(
                format!(
                    " · {} server{}",
                    self.rows.len(),
                    if self.rows.len() == 1 { "" } else { "s" }
                ),
                theme.muted_style(),
            ),
        ];
        if let Some((label, _)) = &self.busy {
            spans.push(Span::styled(format!(" · {label}…"), theme.accent_style()));
        } else if !self.note.is_empty() {
            spans.push(Span::styled(format!(" · {}", self.note), theme.dim_style()));
        }
        let mut out = vec![Line::from(spans)];
        if let Some(error) = &self.error {
            out.push(Line::from(Span::styled(
                format!(" ✕ {}", crate::popup::one_line(error)),
                Style::default().fg(theme.error),
            )));
        }
        out
    }

    fn detail(&self, row: &McpRow, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let field = |label: &str, value: String| {
            Line::from(vec![
                Span::styled(format!("   {label:<10}"), theme.dim_style()),
                Span::styled(clip(&value, width.saturating_sub(14)), theme.muted_style()),
            ])
        };
        let tools = row.tools.map_or_else(
            || "not probed · p lists tools".to_owned(),
            |count| count.to_string(),
        );
        vec![
            field("file", row.path.display().to_string()),
            field("target", row.target.clone()),
            field("transport", row.transport.clone()),
            field("enabled", row.enabled.to_string()),
            field(
                "session",
                format!(
                    "{} · {}",
                    row.session,
                    row.state.as_deref().unwrap_or("none")
                ),
            ),
            field("auth", row.auth().to_owned()),
            field("tools", tools),
        ]
    }
}

impl BottomView for McpPage {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut out = self.header(theme);
        if !self.enabled {
            out.push(Line::from(Span::styled(
                " MCP is off. e turns it on; servers are listed once it is.".to_owned(),
                theme.dim_style(),
            )));
        } else if self.rows.is_empty() && self.error.is_none() {
            out.push(Line::from(Span::styled(
                " no servers in ~/.yi/mcp.json, .mcp.json, .vscode/mcp.json or .cursor/mcp.json"
                    .to_owned(),
                theme.dim_style(),
            )));
        } else if !self.rows.is_empty() {
            out.push(Line::from(Span::styled(
                format!(
                    "   {:<18}{:<7}{:<14}{:>6}   source",
                    "server", "kind", "auth", "tools"
                ),
                theme.muted_style().add_modifier(Modifier::BOLD),
            )));
        }
        for (index, row) in self.rows.iter().enumerate() {
            let selected = index == self.selected;
            let style = if selected {
                theme.accent_style().add_modifier(Modifier::BOLD)
            } else {
                theme.muted_style()
            };
            let tools = row
                .tools
                .map_or_else(|| "—".to_owned(), |count| count.to_string());
            out.push(Line::from(vec![
                Span::styled(if selected { " ▸ " } else { "   " }.to_owned(), style),
                Span::styled(format!("{} ", mark(row)), style),
                Span::styled(format!("{:<16}", clip(&row.name, 15)), style),
                Span::styled(format!("{:<7}", row.transport), theme.muted_style()),
                Span::styled(format!("{:<14}", row.auth()), theme.muted_style()),
                Span::styled(format!("{tools:>6}   "), theme.muted_style()),
                Span::styled(row.source.clone(), theme.dim_style()),
            ]));
        }
        if let Some(row) = self.selected_row() {
            out.extend(self.detail(row, width, theme));
        }
        let keys = if width >= 96 {
            " ↑↓ select · e enable/disable · p probe tools · l login · o open config · r refresh · esc close"
        } else {
            " ↑↓ · e on/off · p probe · l login · o file · r reload · esc"
        };
        out.push(Line::from(Span::styled(
            clip(keys, width),
            theme.dim_style(),
        )));
        out
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        match key.code {
            KeyCodeValue::Esc => return PopupResult::Close,
            KeyCodeValue::Up => self.selected = self.selected.saturating_sub(1),
            KeyCodeValue::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(self.rows.len().saturating_sub(1));
            }
            KeyCodeValue::Char(letter) if self.busy.is_none() => match letter {
                'e' => self.toggle_enabled(),
                'p' => self.probe(),
                'l' => self.login(),
                'o' => self.open_config(),

                'r' => self.reload(),
                _ => {}
            },
            _ => {}
        }
        PopupResult::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_page_probe_key_moves_the_tool_count_to_the_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let row = McpRow {
            name: "fixture".to_owned(),
            source: ".mcp.json".to_owned(),
            path: PathBuf::from(".mcp.json"),
            transport: "stdio".to_owned(),
            target: "fixture".to_owned(),
            enabled: true,
            session: "fixture".to_owned(),
            state: None,
            tools: None,
        };
        let mut page = McpPage::from_rows(None, true, vec![row]);
        page.probe = Some(|row| {
            let mut row = row.clone();
            row.tools = Some(2);
            Ok(vec![row])
        });
        let key = SingleKey::parse("p").map_err(std::io::Error::other)?;
        page.handle_key(&key);
        while page.is_busy() {
            let _ = page.poll();
            std::thread::yield_now();
        }
        assert_eq!(page.rows.first().and_then(|row| row.tools), Some(2));
        Ok(())
    }
}
