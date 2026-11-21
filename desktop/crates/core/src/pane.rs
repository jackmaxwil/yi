//! The kinds of pane that ship built in, plus the one a script draws.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Chat,
    Work,
    Side,
    Shelf,
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
