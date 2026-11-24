//! Design mode's questions to the page the web pane shows: what element is at a point, change
//! one of its styles, put them back. Every answer is the page's own word and is bounded here.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;
use serde_json::{Value, json};
use yd_core::command::table::Run;
use yd_core::command::tools::design::{self, AT, CHANGE, ELEMENT, NONE, Property, RESET, SET};
use yd_core::feed::FeedEvent;
use yd_core::ids::{CommandId, RowId};
use yd_core::row::Row;
use yd_core::tokens::rgb;
#[cfg(any(test, feature = "test-support"))]
use yi_browser::Update;
#[cfg(any(test, feature = "test-support"))]
use yi_types::cdp::{KeyInput, MouseInput};

use crate::browser::{Answer, Page};

/// The one call design mode makes: the page runs [`SCRIPT`] and hands back what it found.
pub const METHOD: &str = "Runtime.evaluate";
/// How long the page may take over one question, in milliseconds.
const PATIENCE: u32 = 500;
/// Questions waiting for the page at once; past it the oldest is forgotten.
const MAX_WAITING: usize = 64;
/// Elements the page keeps as changed; the script refuses a pick past it.
const MAX_ELEMENTS: u32 = 64;
/// The most characters kept of an element's name and of its source location.
const MAX_NAME: usize = 128;
const MAX_SOURCE: usize = 512;
/// Every length the page reports is held within this many pixels of zero.
const MAX_LENGTH: f64 = 1_000_000.0;
/// What the pane is told when the link carries no call.
pub const NO_LINK: &str =
    "This browser cannot be asked about its elements yet: Yi's link sends no Runtime call.";

/// What runs in the page. It keeps the elements it was asked about in `window.__ydDesign`,
/// each with its first style and words, so a reset puts back exactly what the page drew.
const SCRIPT: &str = r#"function(a){
var S=window.__ydDesign||(window.__ydDesign={list:[],cur:-1});
var px=function(v){return parseFloat(v)||0};
if(a.op==='at'){
 var hit=document.elementFromPoint(a.x,a.y);if(!hit)return null;
 var i=-1;for(var k=0;k<S.list.length;k++){if(S.list[k].el===hit)i=k}
 if(i<0){if(S.list.length>=a.max)return {full:true};
  S.list.push({el:hit,style:hit.getAttribute('style'),text:hit.textContent});i=S.list.length-1}
 S.cur=i}
if(a.op==='reset'){S.list.forEach(function(e){
 if(e.style===null)e.el.removeAttribute('style');else e.el.setAttribute('style',e.style);
 if(e.el.children.length===0&&e.el.textContent!==e.text)e.el.textContent=e.text;e.was=null})}
var e=S.list[S.cur];if(!e||!e.el.isConnected)return null;
var el=e.el,st=el.style;
if(a.op==='set'){
 if(a.p==='size'){st.setProperty('box-sizing','border-box');
  st.setProperty('width',a.a+'px');st.setProperty('height',a.b+'px')}
 if(a.p==='padding')st.setProperty('padding',a.a+'px '+a.b+'px');
 if(a.p==='fill')st.setProperty('background-color',a.v);
 if(a.p==='text')st.setProperty('color',a.v);
 if(a.p==='corners')st.setProperty('border-radius',a.a+'px');
 if(a.p==='says'&&el.children.length===0)el.textContent=a.v}
var cs=getComputedStyle(el),r=el.getBoundingClientRect();
var next=el.nextElementSibling,par=el.parentElement;
var cls=typeof el.className==='string'?el.className.trim().split(/\s+/)[0]:'';
var name=el.tagName.toLowerCase()+(el.id?'#'+el.id:(cls?'.'+cls:''));
var src='',fk=Object.keys(el).find(function(k){return k.indexOf('__reactFiber$')===0});
var f=fk?el[fk]:null;
for(var n=0;f&&n<50&&!src;n++){var d=f._debugSource;
 if(d&&d.fileName)src=d.fileName+':'+d.lineNumber;f=f.return}
var now={w:r.width,h:r.height,pv:px(cs.paddingTop),ph:px(cs.paddingLeft),
 fill:cs.backgroundColor,ink:cs.color,radius:px(cs.borderTopLeftRadius),
 says:String(el.textContent||'').slice(0,a.words)};
if(!e.was)e.was=now;
return {n:S.cur,name:name.slice(0,128),source:String(src).slice(0,512),
 leaf:el.children.length===0,x:r.left,y:r.top,
 html:el.outerHTML.slice(0,a.html),html_size:el.outerHTML.length,
 gap:next?next.getBoundingClientRect().top-r.bottom:0,
 inset:par?r.left-par.getBoundingClientRect().left:0,now:now,was:e.was}
}"#;

/// The expression that asks `args` of the page.
fn expression(args: &Value) -> String {
    format!("({SCRIPT})({args})")
}

/// What an expression made by this module asks: its arguments. A stand-in reads them.
pub fn asked(expression: &str) -> Option<Value> {
    let args = expression.strip_prefix('(')?.strip_prefix(SCRIPT)?;
    serde_json::from_str(args.strip_prefix(")(")?.strip_suffix(')')?).ok()
}

/// One element's look, as the page computed it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Look {
    w: f64,
    h: f64,
    pv: f64,
    ph: f64,
    fill: String,
    ink: String,
    radius: f64,
    says: String,
}

/// What the page answers. A field it leaves out is its zero; one of another shape refuses
/// the whole answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Seen {
    full: bool,
    n: f64,
    name: String,
    source: String,
    leaf: bool,
    html: String,
    html_size: f64,
    x: f64,
    y: f64,
    gap: f64,
    inset: f64,
    now: Look,
    was: Look,
}

/// A length from the page as whole pixels: not a number is zero, and none passes the bound.
fn whole(length: f64) -> i64 {
    let held = if length.is_finite() { length } else { 0.0 };
    held.clamp(-MAX_LENGTH, MAX_LENGTH).round() as i64
}

fn side(length: f64) -> u32 {
    u32::try_from(whole(length).max(0)).unwrap_or(0)
}

/// Text from the page as one line of at most `most` characters, cut between characters.
fn line(text: &str, most: usize) -> String {
    let plain = text
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace());
    let words: String = plain.take(most.saturating_mul(4)).collect();
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    words.chars().take(most).collect()
}

/// A computed colour, `rgb(r, g, b)` or `rgba(r, g, b, a)`, as `#rrggbb`; a clear one or any
/// other form is no colour.
fn hex(css: &str) -> String {
    let inner = css
        .strip_prefix("rgba(")
        .or_else(|| css.strip_prefix("rgb("));
    let inner = inner.and_then(|rest| rest.strip_suffix(')')).unwrap_or("");
    let parts: Vec<&str> = inner.split(',').map(str::trim).take(5).collect();
    let byte = |at: usize| parts.get(at).and_then(|part| part.parse::<u8>().ok());
    let clear = parts.get(3).is_some_and(|a| a.parse::<f32>() == Ok(0.0));
    match (byte(0), byte(1), byte(2)) {
        (Some(r), Some(g), Some(b)) if !clear && parts.len() <= 4 => rgb(r, g, b).hex(),
        _ => NONE.to_owned(),
    }
}

impl Look {
    /// This look's value of `property`, in the form [`Property::checked`] keeps.
    fn value(&self, property: Property) -> String {
        match property {
            Property::Size => format!("{} × {}", side(self.w), side(self.h)),
            Property::Padding => format!("{} {}", side(self.pv), side(self.ph)),
            Property::Fill => hex(&self.fill),
            Property::Text => hex(&self.ink),
            Property::Corners => side(self.radius).to_string(),
            Property::Says => line(&self.says, design::MAX_WORDS),
        }
    }
}

/// Where a page is, as an element's address names it: the address less its scheme, query
/// and fragment.
fn place(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    line(path.trim_start_matches('/'), MAX_SOURCE)
}

/// What a question was, kept until the page answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Look,
    Reset,
}

/// Design mode for one page: the questions not yet answered and the change rows sent.
#[derive(Debug, Default)]
pub struct Design {
    next: u64,
    waiting: VecDeque<(u64, Op)>,
    changed: BTreeSet<String>,
    last: Option<Row>,
}

impl Design {
    /// Whether `command` is one this module answers.
    pub fn owns(command: CommandId) -> bool {
        [AT, SET, RESET].contains(&command)
    }

    fn noted(&self, why: &str) -> Vec<FeedEvent> {
        let row = self.last.clone().unwrap_or_else(|| Row::new(ELEMENT));
        vec![FeedEvent::Upsert(row.text("note", why))]
    }

    /// The arguments `run` asks the page with, or why it asks nothing. `at` turns a picture
    /// point into a page point.
    fn args(run: &Run, at: impl Fn(u32, u32) -> (u32, u32)) -> Result<(Op, Value), String> {
        let text = run.text.as_deref().unwrap_or("");
        if run.command == RESET {
            return Ok((Op::Reset, json!({"op": "reset"})));
        }
        if run.command == AT {
            let (x, y) = design::pair(text).ok_or("Not picked: no point was given")?;
            let (x, y) = at(x, y);
            return Ok((
                Op::Look,
                json!({"op": "at", "x": x, "y": y, "max": MAX_ELEMENTS}),
            ));
        }
        let refused = || format!("Not changed: “{}” is not a value", line(text, 80));
        let (slug, value) = text.split_once('=').ok_or_else(refused)?;
        let property = Property::from_slug(slug).ok_or_else(refused)?;
        let value = property.checked(value).ok_or_else(refused)?;
        let (a, b) = design::pair(&value).unwrap_or((value.parse().unwrap_or(0), 0));
        let set = json!({"op": "set", "p": property.slug(), "v": value, "a": a, "b": b});
        Ok((Op::Look, set))
    }

    /// Asks the page what `run` asks. What comes back at once is a note: the ask was refused,
    /// or this link carries no call.
    pub fn ask(
        &mut self,
        page: &mut impl Page,
        run: &Run,
        at: impl Fn(u32, u32) -> (u32, u32),
    ) -> Vec<FeedEvent> {
        let (op, mut args) = match Self::args(run, at) {
            Ok(asked) => asked,
            Err(why) => return self.noted(&why),
        };
        if let Some(args) = args.as_object_mut() {
            args.insert("words".to_owned(), json!(design::MAX_WORDS));
            args.insert("html".to_owned(), json!(design::MAX_HTML));
        }
        self.next = self.next.saturating_add(1);
        let params = json!({
            "expression": expression(&args), "returnByValue": true, "timeout": PATIENCE,
        });
        if !page.call(self.next, METHOD, params) {
            return self.noted(NO_LINK);
        }
        self.waiting.push_back((self.next, op));
        let over = self.waiting.len().saturating_sub(MAX_WAITING);
        self.waiting.drain(..over);
        Vec::new()
    }

    /// The page went elsewhere or loaded again: what was picked and changed there is gone.
    pub fn landed(&mut self) -> Vec<FeedEvent> {
        self.waiting.clear();
        let gone = std::mem::take(&mut self.changed).into_iter();
        let shown = self.last.take().map(|_| ELEMENT.to_owned());
        let rows = gone.chain(shown).map(RowId::new);
        rows.map(FeedEvent::Remove).collect()
    }

    /// Folds the page's answers into rows: the picked element, and a row for each property of
    /// it that is not as the page drew it. `url` is the page's address and `ratio` its fit.
    pub fn heard(&mut self, answers: Vec<Answer>, url: &str, ratio: f64) -> Vec<FeedEvent> {
        let mut events = Vec::new();
        for (id, answer) in answers {
            let Some(at) = self.waiting.iter().position(|(known, _)| *known == id) else {
                continue;
            };
            let Some((_, op)) = self.waiting.remove(at) else {
                continue;
            };
            let seen = match answer.as_ref().map(seen) {
                Ok(Ok(seen)) => seen,
                Ok(Err(why)) => {
                    events.extend(self.noted(why));
                    continue;
                }
                Err(why) => {
                    events.extend(self.noted(&line(why, MAX_SOURCE)));
                    continue;
                }
            };
            if op == Op::Reset {
                let gone = std::mem::take(&mut self.changed).into_iter();
                events.extend(gone.map(|id| FeedEvent::Remove(RowId::new(id))));
            }
            events.extend(self.rows(&seen, url, ratio));
        }
        events
    }

    fn rows(&mut self, seen: &Seen, url: &str, ratio: f64) -> Vec<FeedEvent> {
        let n = side(seen.n).min(MAX_ELEMENTS);
        let name = line(&seen.name, MAX_NAME);
        let source = Some(line(&seen.source, MAX_SOURCE)).filter(|source| !source.is_empty());
        let source = source.unwrap_or_else(|| NONE.to_owned());
        let page = place(url);
        let html = line(&seen.html, design::MAX_HTML);
        let kept = u32::try_from(html.chars().count()).unwrap_or(u32::MAX);
        let ratio = if ratio.is_finite() && ratio > 0.0 {
            ratio
        } else {
            1.0
        };
        let fitted = |css: f64| whole(css * ratio).to_string();
        let mut element = Row::new(ELEMENT)
            .text("n", n.to_string())
            .text("name", name.as_str())
            .text("source", source.as_str())
            .text("page", page.as_str())
            .text("leaf", if seen.leaf { "yes" } else { "no" })
            .text("html", html.as_str())
            .text("html_size", side(seen.html_size).max(kept).to_string())
            .text("x", fitted(seen.x))
            .text("y", fitted(seen.y))
            .text("w", fitted(seen.now.w))
            .text("h", fitted(seen.now.h))
            .text("ratio", format!("{ratio}"))
            .text("gap", whole(seen.gap).to_string())
            .text("inset", whole(seen.inset).to_string())
            .text("note", "");
        let mut events = Vec::new();
        for property in Property::ALL {
            let (was, now) = (seen.was.value(property), seen.now.value(property));
            let id = format!("{CHANGE}{n}:{}", property.slug());
            if was != now {
                let change = Row::new(id.as_str())
                    .text("element", name.as_str())
                    .text("source", source.as_str())
                    .text("page", page.as_str())
                    .text("property", property.slug())
                    .text("was", was.as_str())
                    .text("now", now.as_str());
                self.changed.insert(id);
                events.push(FeedEvent::Upsert(change));
            } else if self.changed.remove(&id) {
                events.push(FeedEvent::Remove(RowId::new(id)));
            }
            element = element
                .text(property.slug(), now)
                .text(&format!("{}_was", property.slug()), was);
        }
        self.last = Some(element.clone());
        events.push(FeedEvent::Upsert(element));
        events
    }
}

/// The element one answer describes, or what to say when it describes none.
fn seen(result: &Value) -> Result<Seen, &'static str> {
    if result.get("exceptionDetails").is_some() {
        return Err("The page did not answer: its script failed or ran out of time.");
    }
    let value = result.pointer("/result/value").filter(|v| !v.is_null());
    let value = value.ok_or("Nothing on the page is there to pick.")?;
    let seen = Seen::deserialize(value).map_err(|_| "The page's answer did not read.")?;
    if seen.full {
        return Err("Too many elements are changed: hand them to the agent or reset first.");
    }
    Ok(seen)
}

#[cfg(any(test, feature = "test-support"))]
/// The one element of a scripted page, in CSS pixels and in Chrome's words for a colour.
#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub source: String,
    pub at: (f64, f64),
    pub size: (f64, f64),
    pub padding: (f64, f64),
    /// Computed colours: `rgb(253, 236, 236)`.
    pub fill: String,
    pub ink: String,
    pub radius: f64,
    pub says: String,
    pub gap: f64,
    pub inset: f64,
}

#[cfg(any(test, feature = "test-support"))]
/// A page of one element that answers design's questions as Chrome would: what tests drive,
/// until Yi's link carries the call. It draws nothing and goes nowhere.
#[derive(Debug, Clone, Default)]
pub struct Scripted {
    now: Element,
    first: Element,
    picked: bool,
    was: Option<Value>,
    answers: Vec<Answer>,
    url: String,
    landed: Vec<Update>,
    /// Answers no call, as a page over Yi's link does today.
    unlinked: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl Scripted {
    pub fn of(element: Element) -> Self {
        Self {
            now: element.clone(),
            first: element,
            ..Self::default()
        }
    }

    /// A page that shows and takes input but answers no call: Chrome over Yi's link.
    pub fn unlinked() -> Self {
        Self {
            unlinked: true,
            ..Self::default()
        }
    }

    fn look(&self) -> Value {
        let e = &self.now;
        json!({
            "w": e.size.0, "h": e.size.1, "pv": e.padding.0, "ph": e.padding.1,
            "fill": e.fill, "ink": e.ink, "radius": e.radius, "says": e.says,
        })
    }

    /// `#rrggbb` as Chrome computes it.
    fn computed(hex: &str) -> String {
        match yd_core::tokens::Rgba::parse(hex) {
            Some(c) => format!("rgb({}, {}, {})", c.r, c.g, c.b),
            None => hex.to_owned(),
        }
    }

    /// The element as the page first drew it.
    fn redraw(&mut self) {
        (self.now, self.was) = (self.first.clone(), None);
    }

    fn answer(&mut self, args: &Value) -> Value {
        let word = |name: &str| args.get(name).and_then(Value::as_str).unwrap_or("");
        let number = |name: &str| args.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        let inside = |v: f64, from: f64, len: f64| v >= from && v < from + len;
        let e = &mut self.now;
        match word("op") {
            "at" => {
                let hit =
                    inside(number("x"), e.at.0, e.size.0) && inside(number("y"), e.at.1, e.size.1);
                if !hit {
                    return Value::Null;
                }
                self.picked = true;
            }
            "reset" => self.redraw(),
            _ => match word("p") {
                "size" => e.size = (number("a"), number("b")),
                "padding" => e.padding = (number("a"), number("b")),
                "fill" => e.fill = Self::computed(word("v")),
                "text" => e.ink = Self::computed(word("v")),
                "corners" => e.radius = number("a"),
                _ => e.says = word("v").to_owned(),
            },
        }
        if !self.picked {
            return Value::Null;
        }
        let now = self.look();
        let was = self.was.get_or_insert_with(|| now.clone()).clone();
        let e = &self.now;
        json!({
            "n": 0, "name": e.name, "source": e.source, "leaf": true,
            "x": e.at.0, "y": e.at.1, "gap": e.gap, "inset": e.inset,
            "now": now, "was": was,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Page for Scripted {
    fn poll(&mut self) -> Vec<Update> {
        std::mem::take(&mut self.landed)
    }
    fn navigate(&mut self, url: &str) {
        self.url = url.to_owned();
        self.landed.push(Update::Url(url.to_owned()));
    }
    fn go(&mut self, _: i64) {}
    /// A page loaded again draws its element as it first did, and nothing is picked.
    fn reload(&mut self) {
        self.redraw();
        self.picked = false;
        self.landed.push(Update::Url(self.url.clone()));
    }
    fn resize(&mut self, _: (u32, u32), _: u32) {}
    fn text(&mut self, _: &str) {}
    fn mouse(&mut self, _: &MouseInput) {}
    fn key(&mut self, _: &KeyInput) {}

    fn call(&mut self, id: u64, method: &str, params: Value) -> bool {
        if self.unlinked {
            return false;
        }
        let expression = params.get("expression").and_then(Value::as_str);
        let answer = match expression.and_then(asked).filter(|_| method == METHOD) {
            Some(args) => Ok(json!({"result": {"type": "object", "value": self.answer(&args)}})),
            None => Err(format!("{method} is not a call this page knows")),
        };
        self.answers.push((id, answer));
        true
    }

    fn answers(&mut self) -> Vec<Answer> {
        std::mem::take(&mut self.answers)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    fn cell<'a>(events: &'a [FeedEvent], row: &str, cell: &str) -> Option<&'a str> {
        let mut rows = events.iter().rev().filter_map(|event| match event {
            FeedEvent::Upsert(found) if found.id.as_str() == row => Some(found),
            _ => None,
        });
        rows.next().and_then(|found| found.words(cell))
    }

    fn look(mut design: Design, value: Value) -> Vec<FeedEvent> {
        design.waiting.push_back((1, Op::Look));
        let answer = Ok(json!({"result": {"type": "object", "value": value}}));
        design.heard(
            vec![(1, answer)],
            "http://localhost:5173/upload?x=1#top",
            2.0,
        )
    }

    #[test]
    fn an_over_long_string_from_the_page_is_cut_at_a_character_boundary() -> Check {
        let long = "é".repeat(100_000);
        let events = look(
            Design::default(),
            json!({"name": long, "source": long, "now": {"says": long}}),
        );
        let count = |name: &str| cell(&events, ELEMENT, name).map(|w| w.chars().count());
        assert_eq!(count("name"), Some(MAX_NAME));
        assert_eq!(count("source"), Some(MAX_SOURCE));
        assert_eq!(count("says"), Some(design::MAX_WORDS));
        assert_eq!(
            cell(&events, ELEMENT, "page"),
            Some("localhost:5173/upload")
        );
        Ok(())
    }

    #[test]
    fn an_elements_html_is_kept_to_its_cap_beside_how_long_it_was() -> Check {
        let html = format!("<ul id=\"files\">{}</ul>", "<li>é</li>".repeat(1_000));
        let size = html.chars().count();
        let events = look(
            Design::default(),
            json!({"name": "ul#files", "html": html, "html_size": size}),
        );
        let kept = cell(&events, ELEMENT, "html").ok_or("no html cell")?;
        assert_eq!(kept.chars().count(), design::MAX_HTML);
        assert!(kept.starts_with("<ul id=\"files\"><li>é</li>"), "{kept}");
        let size = size.to_string();
        assert_eq!(cell(&events, ELEMENT, "html_size"), Some(size.as_str()));
        Ok(())
    }

    #[test]
    fn a_number_from_the_page_is_held_in_bounds_and_a_wrong_shape_is_refused() -> Check {
        let wild = json!({"x": 1e300, "y": -1e300, "gap": 9e99, "n": 1e9,
            "now": {"w": 1e308, "h": -5.0, "radius": 1e12, "fill": "rgb(999, 0, 0)"}});
        let events = look(Design::default(), wild);
        let at = |name: &str| cell(&events, ELEMENT, name);
        assert_eq!(at("x"), Some("1000000"));
        assert_eq!(at("y"), Some("-1000000"));
        assert_eq!(at("gap"), Some("1000000"));
        assert_eq!(at("n"), Some("64"));
        assert_eq!(at("size"), Some("1000000 × 0"));
        assert_eq!(at("fill"), Some(NONE));
        let events = look(Design::default(), json!({"name": 5, "now": []}));
        assert_eq!(
            cell(&events, ELEMENT, "note"),
            Some("The page's answer did not read.")
        );
        assert_eq!(cell(&events, ELEMENT, "name"), None);
        Ok(())
    }

    #[test]
    fn a_computed_colour_reads_as_hex_and_a_clear_one_as_none() -> Check {
        assert_eq!(hex("rgb(253, 236, 236)"), "#fdecec");
        assert_eq!(hex("rgba(138, 75, 0, 0.5)"), "#8a4b00");
        for none in [
            "rgba(0, 0, 0, 0)",
            "color(srgb 1 0 0)",
            "rgb(1,2)",
            "rgb(1,2,3,4,5)",
            "",
        ] {
            assert_eq!(hex(none), NONE, "{none}");
        }
        Ok(())
    }

    #[test]
    fn an_expression_carries_typed_words_as_data_never_as_script() -> Check {
        let said = "\"});alert(1);({\"";
        let run = Run {
            text: Some(format!("says={said}")),
            ..Run::new(SET)
        };
        let (_, args) = Design::args(&run, |x, y| (x, y))?;
        let back = asked(&expression(&args)).ok_or("the expression did not read back")?;
        assert_eq!(back.get("v").and_then(Value::as_str), Some(said));
        let bad = Run {
            text: Some("fill=red;x".to_owned()),
            ..Run::new(SET)
        };
        assert!(Design::args(&bad, |x, y| (x, y)).is_err());
        Ok(())
    }

    #[test]
    fn a_link_that_carries_no_call_says_so_in_the_elements_note() -> Check {
        struct Mute;
        impl Page for Mute {
            fn poll(&mut self) -> Vec<Update> {
                Vec::new()
            }
            fn navigate(&mut self, _: &str) {}
            fn go(&mut self, _: i64) {}
            fn reload(&mut self) {}
            fn resize(&mut self, _: (u32, u32), _: u32) {}
            fn text(&mut self, _: &str) {}
            fn mouse(&mut self, _: &MouseInput) {}
            fn key(&mut self, _: &KeyInput) {}
        }
        let run = Run {
            text: Some("3,4".to_owned()),
            ..Run::new(AT)
        };
        let events = Design::default().ask(&mut Mute, &run, |x, y| (x, y));
        assert_eq!(cell(&events, ELEMENT, "note"), Some(NO_LINK));
        Ok(())
    }
}
