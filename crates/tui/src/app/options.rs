use super::App;

pub struct TuiOptions {
    pub model: yi_types::model::Model,
    pub session_name: String,
    pub cwd: String,
    pub lane: Option<String>,
    pub context_window: u64,
    pub session_dir: String,
    pub keys: Vec<(String, String)>,
    pub initial_prompt: Option<String>,
    /// The streamed reveal's speed as a percentage of the default; 0 paints text on arrival.
    pub pace: u16,
    pub follow: yi_types::config::FollowMode,
}

impl App {
    pub fn follow(&self) -> yi_types::config::FollowMode {
        self.follow
    }

    pub fn set_follow(&mut self, follow: yi_types::config::FollowMode) {
        self.follow = follow;
    }

    pub fn advance_follow(&mut self, rows: usize) {
        let Some((width, mode, pinned, below)) = self.pane_follow else {
            // The inline view holds its own window, so the first PgDn opens the hold there.
            if !self.pane {
                self.pane_follow = Some((self.content_width(), self.mode, 0, rows));
            }
            return;
        };
        self.pane_follow = Some((width, mode, pinned, below.saturating_add(rows)));
    }

    pub fn follow_bottom(&mut self) {
        self.pane_follow = None;
    }
}
