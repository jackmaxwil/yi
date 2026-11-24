//! What a thing on screen is: the first part of its address and the key of its menu items.

macro_rules! kinds {
    ($($name:ident $slug:literal),+ $(,)?) => {
        /// Every kind of thing that can be boxed, attached, right-clicked or addressed.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Kind { $($name),+ }

        impl Kind {
            pub const ALL: &'static [Kind] = &[$(Kind::$name),+];

            /// The kind as it is written in an address.
            pub const fn slug(self) -> &'static str {
                match self { $(Kind::$name => $slug),+ }
            }

            pub fn from_slug(slug: &str) -> Option<Kind> {
                match slug { $($slug => Some(Kind::$name),)+ _ => None }
            }
        }
    };
}

kinds! {
    Lines "lines", File "file", Comment "comment", Step "step", Command "command",
    Approval "approval", Answer "answer", Message "message", Thread "thread", Turn "turn",
    Card "card", Element "element", Screen "screen", Page "page", Agent "agent", Span "span",
    LogLine "log-line", Finding "finding", Asset "asset", Container "container", Vm "vm",
    Service "service", Reference "reference", StackFrame "stack-frame", Variable "variable",
    Breakpoint "breakpoint", Notification "notification", Plugin "plugin", Setting "setting",
    Theme "theme", ThemeValue "theme-value", Number "number", Check "check", Place "place",
    StandingCheck "standing-check", Problem "problem", LanguageServer "language-server",
    Property "property", Tab "tab", Pane "pane",
}

/// Address roots Yi itself reads; no kind may take them.
pub const RESERVED: &[&str] = &["tools", "skills"];
