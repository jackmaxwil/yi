use std::collections::HashMap;
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCodeValue {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SingleKey {
    pub code: KeyCodeValue,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyInput {
    Single(SingleKey),
    Sequence(Vec<SingleKey>),
}

impl SingleKey {
    pub fn from_event(event: &KeyEvent) -> Option<Self> {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        let code = match event.code {
            KeyCode::Char(' ') => KeyCodeValue::Space,
            KeyCode::Char(c) => {
                // Uppercase already encodes shift; storing both would make
                // `G` and `shift-G` distinct bindings for one key press.
                if shift && !ctrl && !alt && c.is_ascii_uppercase() {
                    return Some(SingleKey {
                        code: KeyCodeValue::Char(c),
                        ctrl: false,
                        alt: false,
                        shift: false,
                    });
                }
                KeyCodeValue::Char(c)
            }
            KeyCode::Enter => KeyCodeValue::Enter,
            KeyCode::Esc => KeyCodeValue::Esc,
            KeyCode::Tab => KeyCodeValue::Tab,
            KeyCode::BackTab => {
                return Some(SingleKey {
                    code: KeyCodeValue::Tab,
                    ctrl,
                    alt,
                    shift: true,
                });
            }
            KeyCode::Backspace => KeyCodeValue::Backspace,
            KeyCode::Delete => KeyCodeValue::Delete,
            KeyCode::Up => KeyCodeValue::Up,
            KeyCode::Down => KeyCodeValue::Down,
            KeyCode::Left => KeyCodeValue::Left,
            KeyCode::Right => KeyCodeValue::Right,
            KeyCode::Home => KeyCodeValue::Home,
            KeyCode::End => KeyCodeValue::End,
            KeyCode::PageUp => KeyCodeValue::PageUp,
            KeyCode::PageDown => KeyCodeValue::PageDown,
            KeyCode::F(n) => KeyCodeValue::F(n),
            _ => return None,
        };
        Some(SingleKey {
            code,
            ctrl,
            alt,
            shift: if matches!(code, KeyCodeValue::Char(_)) {
                false
            } else {
                shift
            },
        })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let parts: Vec<&str> = s.split('-').collect();
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        for &part in parts.get(..parts.len().saturating_sub(1)).unwrap_or(&[]) {
            match part.to_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => return Err(format!("unknown modifier: {part}")),
            }
        }
        let key_part = parts.last().copied().unwrap_or("");
        let code = match key_part.to_lowercase().as_str() {
            "enter" | "return" => KeyCodeValue::Enter,
            "esc" | "escape" => KeyCodeValue::Esc,
            "tab" => KeyCodeValue::Tab,
            "backspace" => KeyCodeValue::Backspace,
            "delete" | "del" => KeyCodeValue::Delete,
            "up" => KeyCodeValue::Up,
            "down" => KeyCodeValue::Down,
            "left" => KeyCodeValue::Left,
            "right" => KeyCodeValue::Right,
            "home" => KeyCodeValue::Home,
            "end" => KeyCodeValue::End,
            "pageup" => KeyCodeValue::PageUp,
            "pagedown" => KeyCodeValue::PageDown,
            "space" => KeyCodeValue::Space,
            f if f.starts_with('f') && f.len() > 1 => {
                match f.get(1..).unwrap_or("").parse::<u8>() {
                    Ok(n) if (1..=24).contains(&n) => KeyCodeValue::F(n),
                    Ok(_) => return Err(format!("function key out of range: {key_part}")),
                    Err(_) => return Err(format!("unknown key: {key_part}")),
                }
            }
            _ => {
                let mut chars = key_part.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        if c.is_ascii_uppercase() && !ctrl && !alt {
                            return Ok(SingleKey {
                                code: KeyCodeValue::Char(c),
                                ctrl: false,
                                alt: false,
                                shift: false,
                            });
                        }
                        KeyCodeValue::Char(c)
                    }
                    _ => return Err(format!("unknown key: {key_part}")),
                }
            }
        };
        Ok(SingleKey {
            code,
            ctrl,
            alt,
            shift,
        })
    }
}

impl fmt::Display for SingleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "ctrl-")?;
        }
        if self.alt {
            write!(f, "alt-")?;
        }
        if self.shift {
            write!(f, "shift-")?;
        }
        match &self.code {
            KeyCodeValue::Char(c) => write!(f, "{c}"),
            KeyCodeValue::Enter => write!(f, "enter"),
            KeyCodeValue::Esc => write!(f, "esc"),
            KeyCodeValue::Tab => write!(f, "tab"),
            KeyCodeValue::Backspace => write!(f, "backspace"),
            KeyCodeValue::Delete => write!(f, "delete"),
            KeyCodeValue::Up => write!(f, "up"),
            KeyCodeValue::Down => write!(f, "down"),
            KeyCodeValue::Left => write!(f, "left"),
            KeyCodeValue::Right => write!(f, "right"),
            KeyCodeValue::Home => write!(f, "home"),
            KeyCodeValue::End => write!(f, "end"),
            KeyCodeValue::PageUp => write!(f, "pageup"),
            KeyCodeValue::PageDown => write!(f, "pagedown"),
            KeyCodeValue::Space => write!(f, "space"),
            KeyCodeValue::F(n) => write!(f, "f{n}"),
        }
    }
}

impl KeyInput {
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() > 1 {
            let keys: Result<Vec<SingleKey>, String> =
                parts.iter().map(|p| SingleKey::parse(p)).collect();
            Ok(KeyInput::Sequence(keys?))
        } else {
            Ok(KeyInput::Single(SingleKey::parse(s)?))
        }
    }
}

impl fmt::Display for KeyInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyInput::Single(k) => write!(f, "{k}"),
            KeyInput::Sequence(keys) => {
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{k}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Submit,
    InsertNewline,
    Abort,
    Quit,
    HistoryPrev,
    HistoryNext,
    ToggleExpand,
    ToggleHud,
    ExternalEditor,
    FocusChild,
    FocusParent,
    FocusNextSibling,
    FocusPrevSibling,
    RaiseEffort,
    LowerEffort,
    CycleModel,
    CycleModelBack,
    OpenModelPicker,
}

impl Action {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "submit" => Ok(Self::Submit),
            "insert-newline" => Ok(Self::InsertNewline),
            "abort" => Ok(Self::Abort),
            "quit" => Ok(Self::Quit),
            "history-prev" => Ok(Self::HistoryPrev),
            "history-next" => Ok(Self::HistoryNext),
            "toggle-expand" => Ok(Self::ToggleExpand),
            "toggle-hud" => Ok(Self::ToggleHud),
            "external-editor" => Ok(Self::ExternalEditor),
            "focus-child" => Ok(Self::FocusChild),
            "focus-parent" => Ok(Self::FocusParent),
            "focus-next-sibling" => Ok(Self::FocusNextSibling),
            "focus-prev-sibling" => Ok(Self::FocusPrevSibling),
            "raise-effort" => Ok(Self::RaiseEffort),
            "lower-effort" => Ok(Self::LowerEffort),
            "cycle-model" => Ok(Self::CycleModel),
            "cycle-model-back" => Ok(Self::CycleModelBack),
            "open-model-picker" => Ok(Self::OpenModelPicker),
            _ => Err(format!("unknown action: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    InputEmpty,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EvalContext {
    pub input_empty: bool,
}

impl Condition {
    fn evaluate(self, ctx: &EvalContext) -> bool {
        match self {
            Condition::InputEmpty => ctx.input_empty,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub when: Option<Condition>,
    pub action: Action,
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    pub map: HashMap<KeyInput, Vec<Rule>>,
}

impl Keymap {
    pub fn bind(&mut self, key: KeyInput, action: Action) {
        self.map.insert(key, vec![Rule { when: None, action }]);
    }

    pub fn resolve(&self, key: &KeyInput, ctx: &EvalContext) -> Option<Action> {
        let rules = self.map.get(key)?;
        for rule in rules {
            match rule.when {
                None => return Some(rule.action),
                Some(cond) if cond.evaluate(ctx) => return Some(rule.action),
                Some(_) => {}
            }
        }
        None
    }

    pub fn apply_overrides<'a, I>(&mut self, overrides: I) -> Result<(), String>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        for (key, action) in overrides {
            let key = KeyInput::parse(key)?;
            let action = Action::parse(action)?;
            self.bind(key, action);
        }
        Ok(())
    }
}

fn single(code: KeyCodeValue, ctrl: bool, alt: bool) -> KeyInput {
    KeyInput::Single(SingleKey {
        code,
        ctrl,
        alt,
        shift: false,
    })
}

pub fn default_keymap() -> Keymap {
    let mut map = Keymap::default();
    map.bind(single(KeyCodeValue::Enter, false, false), Action::Submit);
    map.map.insert(
        KeyInput::Single(SingleKey {
            code: KeyCodeValue::Enter,
            ctrl: false,
            alt: false,
            shift: true,
        }),
        vec![Rule {
            when: None,
            action: Action::InsertNewline,
        }],
    );
    map.bind(single(KeyCodeValue::Esc, false, false), Action::Abort);
    map.bind(single(KeyCodeValue::Char('c'), true, false), Action::Quit);
    map.bind(
        single(KeyCodeValue::Char('o'), true, false),
        Action::ToggleExpand,
    );
    map.bind(
        single(KeyCodeValue::Char('t'), true, false),
        Action::ToggleHud,
    );
    map.bind(
        single(KeyCodeValue::Char('g'), true, false),
        Action::ExternalEditor,
    );
    map.bind(single(KeyCodeValue::Down, false, true), Action::FocusChild);
    map.bind(single(KeyCodeValue::Up, false, true), Action::FocusParent);
    map.bind(
        single(KeyCodeValue::Right, false, true),
        Action::FocusNextSibling,
    );
    map.map.insert(
        KeyInput::Single(SingleKey {
            code: KeyCodeValue::Tab,
            ctrl: false,
            alt: false,
            shift: true,
        }),
        vec![Rule {
            when: None,
            action: Action::RaiseEffort,
        }],
    );
    map.bind(single(KeyCodeValue::Tab, false, true), Action::LowerEffort);
    map.bind(
        single(KeyCodeValue::Char('p'), true, false),
        Action::CycleModel,
    );
    map.map.insert(
        KeyInput::Single(SingleKey {
            code: KeyCodeValue::Char('P'),
            ctrl: true,
            alt: false,
            shift: false,
        }),
        vec![Rule {
            when: None,
            action: Action::CycleModelBack,
        }],
    );
    map.bind(
        single(KeyCodeValue::Char('m'), false, true),
        Action::OpenModelPicker,
    );
    map.bind(
        single(KeyCodeValue::Left, false, true),
        Action::FocusPrevSibling,
    );
    map
}
