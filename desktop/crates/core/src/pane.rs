//! The kinds of pane that ship built in, plus the one a script draws.

use std::collections::BTreeMap;

macro_rules! panes {
    ($($name:ident $slug:literal $title:literal),+ $(,)?) => {
        /// A built-in pane kind. A pane's module, scenes and fixtures are named by its slug.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum PaneKind { $($name),+ }

        impl PaneKind {
            pub const ALL: &'static [PaneKind] = &[$(PaneKind::$name),+];

            pub const fn slug(self) -> &'static str {
                match self { $(PaneKind::$name => $slug),+ }
            }

            /// Whether the kind is about this Mac or the app, not a session: a pane of it
            /// the person opens shows in every session of its window.
            pub const fn wide(self) -> bool {
                use PaneKind::{Assets, Containers, Daemons, LanguageServers, Plugins, Settings, Themes, Vms};
                matches!(
                    self,
                    Assets | Containers | Daemons | LanguageServers | Plugins | Settings | Themes | Vms
                )
            }

            /// Whether a window opens a pane of the kind by itself, as a quiet tab, the first
            /// time a session has something of it to show: the rules of what arrives.
            pub const fn arrives(self) -> bool {
                use PaneKind::{Agents, Changes, IPython, Web};
                matches!(self, Changes | Agents | Web | IPython)
            }

            /// [`PaneKind::place`] for a pane about one thing or about none: a chat about
            /// one thing, a child agent's, is work beside the session's own chat.
            pub const fn place_about(self, thing: bool) -> Place {
                if thing && matches!(self, PaneKind::Chat) {
                    Place::Work
                } else {
                    self.place()
                }
            }

            /// Where a pane of the kind goes when it is opened: the stack it joins.
            pub const fn place(self) -> Place {
                use PaneKind::*;
                match self {
                    Chat => Place::Chat,
                    Terminal | Logs | Trace | LanguageServers => Place::Shelf,
                    Files | Agents | Findings | Assets | Containers | Vms | Daemons
                    | Plugins => Place::Side,
                    Changes | Code | Web | Remote | Report | Design | Debugger | Settings
                    | Themes | PluginKit | Script | IPython => Place::Work,
                }
            }

            /// The name a tab and the pane's placeholder show.
            pub const fn title(self) -> &'static str {
                match self { $(PaneKind::$name => $title),+ }
            }
        }
    };
}

/// The four places a window's panes stack in: the chat, the work beside it, a narrow list
/// column, and a short shelf along the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Chat,
    Work,
    Side,
    Shelf,
}

impl Place {
    pub const ALL: [Place; 4] = [Place::Chat, Place::Work, Place::Side, Place::Shelf];

    /// The place a settings line or the grid's file names by `slug`.
    pub fn named(slug: &str) -> Option<Place> {
        Place::ALL
            .iter()
            .copied()
            .find(|place| place.slug() == slug)
    }

    /// The word a settings line names the place by.
    pub const fn slug(self) -> &'static str {
        match self {
            Place::Chat => "chat",
            Place::Work => "work",
            Place::Side => "side",
            Place::Shelf => "shelf",
        }
    }
}

/// The person's word on a kind of pane opening by itself, over the kind's own rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opens {
    /// No rule opens it.
    Never,
    /// It is open in this place in every session, with something to show or not.
    Always(Place),
}

impl Opens {
    const NEVER: &'static str = "never";

    /// The words of a settings text, a line a kind: `<pane> never` or `<pane> <place>`. The
    /// chat has no word, and a line that says neither is passed over.
    pub fn read(text: &str) -> BTreeMap<PaneKind, Opens> {
        let word = |line: &str| {
            let (pane, word) = line.trim().split_once(' ')?;
            let kind = PaneKind::ALL.iter().find(|kind| kind.slug() == pane)?;
            let place = Place::named(word.trim());
            let word = match place {
                Some(place) => Opens::Always(place),
                None if word.trim() == Self::NEVER => Opens::Never,
                None => return None,
            };
            (*kind != PaneKind::Chat).then_some((*kind, word))
        };
        text.lines().filter_map(word).collect()
    }

    /// The text [`Opens::read`] reads `words` back from.
    pub fn write(words: &BTreeMap<PaneKind, Opens>) -> String {
        let line = |(kind, word): (&PaneKind, &Opens)| {
            let word = match word {
                Opens::Never => Self::NEVER,
                Opens::Always(place) => place.slug(),
            };
            format!("{} {word}", kind.slug())
        };
        words.iter().map(line).collect::<Vec<_>>().join("\n")
    }
}

panes! {
    Chat "chat" "Chat", Files "files" "Files", Changes "changes" "Changes",
    Code "code" "Code", Terminal "terminal" "Terminal", Web "web" "Web app",
    Remote "remote" "Remote desktop", Report "report" "Report", Design "design" "Design",
    Agents "agents" "Agents", Trace "trace" "Trace", Logs "logs" "Logs",
    Findings "findings" "Findings", Assets "assets" "Assets",
    Containers "containers" "Containers", Vms "vms" "VMs", Daemons "daemons" "Daemons",
    Debugger "debugger" "Debugger", LanguageServers "lsp" "Language servers",
    Plugins "plugins" "Plugins", Settings "settings" "Settings", Themes "themes" "Themes",
    PluginKit "plugin-kit" "Plugin kit", Script "script" "Script", IPython "ipython" "IPython",
}
