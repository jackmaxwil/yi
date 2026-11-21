//! The kinds of pane that ship built in, plus the one a script draws.

macro_rules! panes {
    ($($name:ident $slug:literal $title:literal $wide:literal),+ $(,)?) => {
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
                match self { $(PaneKind::$name => $wide),+ }
            }

            /// The name a tab and the pane's placeholder show.
            pub const fn title(self) -> &'static str {
                match self { $(PaneKind::$name => $title),+ }
            }
        }
    };
}

panes! {
    Chat "chat" "Chat" false, Files "files" "Files" false, Changes "changes" "Changes" false,
    Code "code" "Code" false, Terminal "terminal" "Terminal" false, Web "web" "Web app" false,
    Remote "remote" "Remote desktop" false, Report "report" "Report" false, Design "design" "Design" false,
    Agents "agents" "Agents" false, Trace "trace" "Trace" false, Logs "logs" "Logs" false,
    Findings "findings" "Findings" false, Assets "assets" "Assets" true,
    Containers "containers" "Containers" true, Vms "vms" "VMs" true, Daemons "daemons" "Daemons" true,
    Debugger "debugger" "Debugger" false, LanguageServers "lsp" "Language servers" true,
    Plugins "plugins" "Plugins" true, Settings "settings" "Settings" true, Themes "themes" "Themes" true,
    PluginKit "plugin-kit" "Plugin kit" false, Script "script" "Script" false, IPython "ipython" "IPython" false,
}
