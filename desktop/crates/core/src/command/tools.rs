//! The command ids the code-tool sources answer to, named once for the source and its panes.

/// The `lsp` source's commands.
pub mod lsp {
    use crate::ids::CommandId;

    pub const OPEN: CommandId = CommandId("lsp.open");
    pub const HOVER: CommandId = CommandId("lsp.hover");
    pub const DEFINITION: CommandId = CommandId("lsp.definition");
    pub const REFERENCES: CommandId = CommandId("lsp.references");
    pub const HINTS: CommandId = CommandId("lsp.hints");
    pub const FIXES: CommandId = CommandId("lsp.fixes");
    pub const APPLY_FIX: CommandId = CommandId("lsp.apply-fix");
    pub const RESTART: CommandId = CommandId("lsp.restart");
}

/// The `dap` source's commands.
pub mod debug {
    use crate::ids::CommandId;

    pub const LAUNCH: CommandId = CommandId("debug.launch");
    pub const ATTACH: CommandId = CommandId("debug.attach");
    pub const CONTINUE: CommandId = CommandId("debug.continue");
    pub const STEP_OVER: CommandId = CommandId("debug.step-over");
    pub const STEP_IN: CommandId = CommandId("debug.step-in");
    pub const STEP_OUT: CommandId = CommandId("debug.step-out");
    pub const RESTART: CommandId = CommandId("debug.restart");
    pub const STOP: CommandId = CommandId("debug.stop");
    pub const BREAK: CommandId = CommandId("debug.break");
    pub const WATCH: CommandId = CommandId("debug.watch");
    pub const EVAL: CommandId = CommandId("debug.eval");
    /// Fetches a `var:` row's children; aimed at its `variable` address.
    pub const EXPAND: CommandId = CommandId("debug.expand");
}

/// The `git` source's commands.
pub mod changes {
    use crate::ids::CommandId;

    /// Merges the shown Yi checkout's branch into its base, in the project's own folder.
    pub const MERGE: CommandId = CommandId("changes.merge");
}

/// The `docker` source's commands.
pub mod container {
    use crate::ids::CommandId;

    pub const LOGS: CommandId = CommandId("container.logs");
    pub const SHELL: CommandId = CommandId("container.shell");
    pub const RESTART: CommandId = CommandId("container.restart");
    pub const STOP: CommandId = CommandId("container.stop");
    pub const START: CommandId = CommandId("container.start");
    pub const REMOVE: CommandId = CommandId("container.remove");
}

/// The `launchd` source's commands.
pub mod service {
    use crate::ids::CommandId;

    pub const START: CommandId = CommandId("service.start");
    pub const STOP: CommandId = CommandId("service.stop");
    pub const RESTART: CommandId = CommandId("service.restart");
    pub const LOGS: CommandId = CommandId("service.logs");
    pub const LOGIN: CommandId = CommandId("service.login");
}

/// The `browser` source's commands.
pub mod web {
    use crate::ids::CommandId;

    pub const OPEN: CommandId = CommandId("web.open");
    pub const BACK: CommandId = CommandId("web.back");
    pub const FORWARD: CommandId = CommandId("web.forward");
    pub const RELOAD: CommandId = CommandId("web.reload");
    pub const WIDTH: CommandId = CommandId("web.width");

    /// The longest address a page is sent to, in bytes.
    pub const MAX_ADDRESS: usize = 2_048;

    /// An address a page may be sent to: `http`, `https`, `file`, or a bare `host[:port][/…]`
    /// read as `http`. Any other, one over [`MAX_ADDRESS`] or with a space or control is not.
    pub fn address(typed: &str) -> Option<String> {
        let typed = typed.trim();
        let bad = |c: char| c.is_whitespace() || c.is_control();
        if typed.is_empty() || typed.len() > MAX_ADDRESS || typed.chars().any(bad) {
            return None;
        }
        let lower = typed.to_ascii_lowercase();
        for scheme in ["http://", "https://"] {
            if let Some(rest) = lower.strip_prefix(scheme) {
                return host(rest).then(|| typed.to_owned());
            }
        }
        if lower.starts_with("file:///") {
            return Some(typed.to_owned());
        }
        (!lower.contains("://") && host(&lower)).then(|| format!("http://{typed}"))
    }

    /// Whether `rest` opens with a host name and an optional port of digits.
    fn host(rest: &str) -> bool {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (name, port) = authority.split_once(':').unwrap_or((authority, "80"));
        let plain = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
        let named = !name.is_empty() && name.chars().all(plain);
        named && !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit())
    }

    #[cfg(test)]
    mod tests {
        use super::{MAX_ADDRESS, address};

        #[test]
        fn a_typed_address_that_is_not_a_web_address_is_refused() {
            assert_eq!(
                address("localhost:5173/upload").as_deref(),
                Some("http://localhost:5173/upload")
            );
            assert_eq!(
                address("https://example.com/a?b").as_deref(),
                Some("https://example.com/a?b")
            );
            assert_eq!(
                address("file:///tmp/r.html").as_deref(),
                Some("file:///tmp/r.html")
            );
            for refused in [
                "javascript:alert(1)",
                "data:text/html,hi",
                "ftp://x",
                "a b",
                "",
                "chrome://settings",
                "http://a\u{7}",
            ] {
                assert_eq!(address(refused), None, "{refused}");
            }
            let long = format!("http://a/{}", "x".repeat(MAX_ADDRESS));
            assert_eq!(address(&long), None);
        }
    }
}

/// The `fetch` source's command.
pub mod fetch {
    use crate::ids::CommandId;

    pub const GET: CommandId = CommandId("fetch.get");
}

/// The command that has Yi write a script pane.
pub mod plugin {
    use crate::ids::CommandId;

    pub const BUILD: CommandId = CommandId("plugin.build");
    /// Reads a plugin's files again; aimed at the plugin, or at a number or row it drew.
    pub const RELOAD: CommandId = CommandId("plugins.reload");
    /// Opens a plugin's `pane.luau` in the code viewer.
    pub const OPEN_SOURCE: CommandId = CommandId("plugins.open-source");
}

/// Design mode: what the web pane and the Design pane ask of the `browser` source, and the
/// values both sides check.
pub mod design {
    use crate::ids::CommandId;
    use crate::tokens::Rgba;

    /// Turns design mode on for the page; aimed at an element, that element is the one picked.
    pub const START: CommandId = CommandId("design.start");
    pub const STOP: CommandId = CommandId("design.stop");
    /// The next press on the page picks the element under it.
    pub const PICK: CommandId = CommandId("design.pick");
    /// Sends the thread the change list, each changed element and its source attached.
    pub const HAND: CommandId = CommandId("design.hand");
    /// Puts every changed element back as the page drew it.
    pub const RESET: CommandId = CommandId("design.reset");
    /// Asks what is under a point: the text is `x,y` in the picture's pixels.
    pub const AT: CommandId = CommandId("design.at");
    /// Changes the picked element: the text is `<property>=<value>`.
    pub const SET: CommandId = CommandId("design.set");

    /// The row of the page's feed that describes the picked element.
    pub const ELEMENT: &str = "element";
    /// What a change's row id starts with: `change:<element number>:<property>`.
    pub const CHANGE: &str = "change:";
    /// What stands for a value the page does not give: no source location, no fill.
    pub const NONE: &str = "–";
    /// The most characters an element is given to say, and the most kept of what it says.
    pub const MAX_WORDS: usize = 512;
    /// The most characters kept of an element's HTML.
    pub const MAX_HTML: usize = 2_000;
    /// The widest and tallest an element is made, in CSS pixels.
    pub const MAX_SIDE: u32 = 8_192;
    /// The largest padding and corner radius, in CSS pixels.
    pub const MAX_INSET: u32 = 512;

    /// What design mode changes of an element, in the order the pane lists them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Property {
        Size,
        Padding,
        Fill,
        Text,
        Corners,
        Says,
    }

    impl Property {
        pub const ALL: [Property; 6] = [
            Property::Size,
            Property::Padding,
            Property::Fill,
            Property::Text,
            Property::Corners,
            Property::Says,
        ];

        /// The name of its cell, and of the property in a `design.set`.
        pub const fn slug(self) -> &'static str {
            match self {
                Property::Size => "size",
                Property::Padding => "padding",
                Property::Fill => "fill",
                Property::Text => "text",
                Property::Corners => "corners",
                Property::Says => "says",
            }
        }

        pub fn from_slug(slug: &str) -> Option<Property> {
            Property::ALL.into_iter().find(|p| p.slug() == slug)
        }

        /// What the list of changes calls it.
        pub const fn noun(self) -> &'static str {
            match self {
                Property::Size => "size",
                Property::Padding => "padding",
                Property::Fill => "fill",
                Property::Text => "text colour",
                Property::Corners => "corners",
                Property::Says => "words",
            }
        }

        /// A typed value as this property keeps and shows it: `602 × 30`, `7 10`, `#fff4e5`,
        /// `10`, the words. None for what is not a value of it; a kept value checks as itself.
        pub fn checked(self, typed: &str) -> Option<String> {
            let typed = typed.trim();
            match self {
                Property::Size => {
                    let (w, h) = pair(typed).filter(|(w, h)| *w > 0 && *h > 0)?;
                    (w <= MAX_SIDE && h <= MAX_SIDE).then(|| format!("{w} × {h}"))
                }
                Property::Padding => {
                    let one = || typed.parse::<u32>().ok().map(|n| (n, n));
                    let (v, h) = pair(typed).or_else(one)?;
                    (v <= MAX_INSET && h <= MAX_INSET).then(|| format!("{v} {h}"))
                }
                Property::Fill | Property::Text => colour(typed),
                Property::Corners => {
                    let radius = typed.parse::<u32>().ok()?;
                    (radius <= MAX_INSET).then(|| radius.to_string())
                }
                Property::Says => {
                    let fits = typed.chars().count() <= MAX_WORDS;
                    let plain = !typed.chars().any(char::is_control);
                    (!typed.is_empty() && fits && plain).then(|| typed.to_owned())
                }
            }
        }
    }

    /// Two whole numbers with `×`, `x`, a comma or spaces between them.
    pub fn pair(text: &str) -> Option<(u32, u32)> {
        let between = |c: char| c == '×' || c == 'x' || c == ',' || c.is_whitespace();
        let mut parts = text.split(between).filter(|part| !part.is_empty());
        let (a, b) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        parts.next().is_none().then_some((a, b))
    }

    /// `#rgb` or `#rrggbb` as `#rrggbb` in lower case; nothing else is a colour here.
    pub fn colour(typed: &str) -> Option<String> {
        let hex = typed.strip_prefix('#')?;
        let full: String = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 => hex.to_owned(),
            _ => return None,
        };
        Rgba::parse(&format!("#{full}")).map(Rgba::hex)
    }

    #[cfg(test)]
    mod tests {
        use std::error::Error;

        use super::{MAX_SIDE, MAX_WORDS, Property, colour, pair};

        #[test]
        fn a_typed_value_is_kept_only_when_it_is_a_value_of_its_property()
        -> Result<(), Box<dyn Error>> {
            let kept = |p: Property, typed: &str| p.checked(typed);
            assert_eq!(kept(Property::Size, "602x30").as_deref(), Some("602 × 30"));
            assert_eq!(kept(Property::Size, "0 × 30"), None);
            assert_eq!(kept(Property::Size, &format!("{} 1", MAX_SIDE + 1)), None);
            assert_eq!(kept(Property::Padding, "7, 10").as_deref(), Some("7 10"));
            assert_eq!(kept(Property::Padding, "8").as_deref(), Some("8 8"));
            assert_eq!(kept(Property::Fill, "#FFF4E5").as_deref(), Some("#fff4e5"));
            assert_eq!(kept(Property::Text, "#abc").as_deref(), Some("#aabbcc"));
            for bad in ["fff4e5", "#fff4e", "#ggg", "red", "#fff4e5ff", "#ééé", ""] {
                assert_eq!(colour(bad), None, "{bad}");
            }
            assert_eq!(kept(Property::Corners, "10").as_deref(), Some("10"));
            assert_eq!(kept(Property::Corners, "-1"), None);
            assert_eq!(kept(Property::Corners, "513"), None);
            assert_eq!(
                kept(Property::Says, " Slow down. ").as_deref(),
                Some("Slow down.")
            );
            assert_eq!(kept(Property::Says, "a\u{7}b"), None);
            assert_eq!(kept(Property::Says, &"é".repeat(MAX_WORDS + 1)), None);
            assert_eq!(pair("1 2 3"), None);
            Ok(())
        }

        #[test]
        fn a_kept_value_checks_as_itself_so_the_source_reads_what_the_pane_sent()
        -> Result<(), Box<dyn Error>> {
            let typed = ["602x30", "7 10", "#fff4e5", "#8a4b00", "10", "Slow down."];
            for (property, typed) in Property::ALL.into_iter().zip(typed) {
                let kept = property.checked(typed);
                let again = kept.as_deref().and_then(|kept| property.checked(kept));
                assert!(kept.is_some() && kept == again, "{property:?}: {kept:?}");
                assert_eq!(Property::from_slug(property.slug()), Some(property));
            }
            Ok(())
        }
    }
}
/// The `machines` source's own asks, the Remote desktop pane's buttons, and the recorded screen.
pub mod remote;
