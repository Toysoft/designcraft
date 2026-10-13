//! The command registry. Ids follow InDesign's menu structure.

mod anchored;
pub mod anchors;
mod articles;
pub mod book;
mod buttons;
pub(crate) mod captions;
mod changes;
mod colorsettings;
mod conditions;
mod datamerge;
mod edit;
mod editorial;
mod endnotes;
mod export;
mod file;
mod find;
pub(crate) mod fonts;
mod guides;
mod index;
mod inspect;
mod interactive;
pub mod interchange;
mod layout;
pub mod library;
mod linked;
mod links;
mod liquid;
mod lists;
mod mathexpr;
mod media;
mod notes;
mod object;
mod overrides;
mod package;
mod path_type;
mod paths;
mod pdflayers;
mod place_text;
pub mod preflight;
mod prefs;
mod print;
mod qr;
mod scripts;
mod select;
pub mod spelling;
mod states;
mod strokes;
mod style;
pub mod table;
pub mod text;
mod toc;
mod transitions;
mod variables;
mod xml;
mod xref;

use designcraft_doc::{ItemId, SpreadRef};
use designcraft_geom::{Point, Rect};
use serde::Serialize;
use serde_json::Value;

use crate::{EngineError, Result, Session};

pub type Run = fn(&mut Session, &Value) -> Result<Value>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Object", "Arrange"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    /// The parameters, in the grammar [`parse_params`] reads: also the keys [`Session::call`]
    /// accepts.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    pub journal: bool,
    /// Record an undo step when the document changes.
    pub undoable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Vec<&'static str>,
    pub shortcut: Option<&'static str>,
    pub params: &'static str,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl CommandSpec {
    pub fn info(&self, s: &Session) -> CommandInfo {
        let e = (self.enabled)(s);
        CommandInfo {
            id: self.id,
            label: self.label,
            menu: self.menu.to_vec(),
            shortcut: self.shortcut,
            params: self.params,
            enabled: e.is_ok(),
            disabled_reason: e.err(),
        }
    }
}

pub fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
/// Why [`has_doc`] disables a command.
pub const NO_DOCUMENT: &str = "no document open";
pub fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| NO_DOCUMENT.into())
}
/// Why [`has_selection`] disables a command. A command that documents `ids` still runs when the
/// call names its objects (see [`named_targets`]).
pub const NOTHING_SELECTED: &str = "nothing selected";
pub fn has_selection(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.active().is_some_and(|d| !d.selection.items.is_empty()) { Ok(()) } else { Err(NOTHING_SELECTED.into()) }
}
/// Why [`has_text`] disables a command.
pub const NO_TEXT_POINT: &str = "no text insertion point";
pub fn has_text(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.active().is_some_and(|d| d.selection.text.is_some()) { Ok(()) } else { Err(NO_TEXT_POINT.into()) }
}
/// Why [`has_text_or_frames`] disables a command.
pub const SELECT_TEXT_OR_FRAME: &str = "select text or a text frame";
pub fn has_text_or_frames(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.active().is_some_and(|d| d.selection.text.is_some() || d.selection.items.iter().any(|i| d.doc.item(*i).is_some_and(|x| x.is_text_frame()))) {
        Ok(())
    } else {
        Err(SELECT_TEXT_OR_FRAME.into())
    }
}

/// The sentence after a "not available" reason: the call that makes `cmd` available.
pub(crate) fn how_to_enable(cmd: &str, reason: &str) -> String {
    match reason {
        NO_DOCUMENT => " file.new or file.open opens one.".into(),
        NOTHING_SELECTED => {
            let acts = find_command(cmd).and_then(|c| c.acts_on_for(reason)).map(|a| format!(" {a}")).unwrap_or_default();
            format!("{acts} selection.set {{\"ids\": […]}} selects objects; frame.create selects the frame it creates.")
        }
        NO_TEXT_POINT => {
            " text.select {\"story\": …, \"anchor\": 0, \"focus\": 0} puts the caret in a story; frame.create with \"content\": \"text\" puts it in the new frame."
                .into()
        }
        SELECT_TEXT_OR_FRAME => " selection.set {\"ids\": […]} selects frames; text.select puts the caret in a story.".into(),
        _ => String::new(),
    }
}

impl CommandSpec {
    /// What the command acts on when it needs a selection, as a sentence (`Acts on the objects in
    /// `ids`, else on the selected objects.`). `None` for a command that needs no selection.
    pub fn acts_on(&self) -> Option<String> {
        self.acts_on_for(&(self.enabled)(&Session::with_new()).err()?)
    }

    pub(crate) fn acts_on_for(&self, reason: &str) -> Option<String> {
        let keys = param_keys(self.params).unwrap_or_default();
        let named = if keys.contains(&"ids") {
            Some("the objects in `ids`")
        } else if keys.contains(&"id") {
            Some("the object in `id`")
        } else {
            None
        };
        Some(match (reason, named) {
            (NOTHING_SELECTED, Some(n)) => format!("Acts on {n}, else on the selected objects."),
            (NOTHING_SELECTED, None) => "Acts on the selected objects.".into(),
            (NO_TEXT_POINT, _) => "Acts on the selected text or at the caret.".into(),
            (SELECT_TEXT_OR_FRAME, _) => "Acts on the selected text, else on the selected text frames.".into(),
            _ => return None,
        })
    }
}

/// Up to eight command ids an agent may have meant by `id`: those containing it, those whose last
/// segment starts like its last segment, and those within two edits of it.
pub fn similar_commands(id: &str) -> Vec<&'static str> {
    let q = id.to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let last = q.rsplit('.').next().unwrap_or_default();
    command_specs()
        .iter()
        .map(|c| c.id)
        .filter(|c| {
            let c = c.to_lowercase();
            c.contains(&q) || c.rsplit('.').next().is_some_and(|l| last.len() >= 3 && l.starts_with(last)) || edit_distance(&c, &q) <= 2
        })
        .take(8)
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let best = (prev.get(j + 1).copied().unwrap_or(usize::MAX).saturating_add(1))
                .min(cur.get(j).copied().unwrap_or(usize::MAX).saturating_add(1))
                .min(prev.get(j).copied().unwrap_or(usize::MAX).saturating_add(cost));
            cur.push(best);
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(usize::MAX)
}

/// The top-level keys a params doc names, each once, in order of first appearance.
pub fn param_keys(doc: &str) -> std::result::Result<Vec<&str>, String> {
    let mut keys = vec![];
    for p in parse_params(doc)? {
        if !keys.contains(&p.name) {
            keys.push(p.name);
        }
    }
    Ok(keys)
}

/// One top-level parameter of a params doc, as [`parse_params`] reads it.
#[derive(Debug, PartialEq)]
pub struct Param<'a> {
    /// `…` names no parameter: the object takes other keys too.
    pub name: &'a str,
    pub optional: bool,
    /// What the doc says of it: its values, unit or remark (`rectangle|ellipse`, `[x,y] (spread coords)`).
    pub about: &'a str,
    /// An alternative to the parameter before it in the same object (`path? | base64?`).
    pub or: bool,
    /// Which of the doc's alternative objects it belongs to: `{index} | {id}` puts `id` in object 1.
    pub object: usize,
}

/// The top-level parameters of a params doc: `{spread?, rect: [x0,y0,x1,y1] (spread coords)} | {id}`
/// gives `spread` (optional), `rect` (about `[x0,y0,x1,y1] (spread coords)`) and `id` (in object 1).
/// A spaced ` | ` separates alternatives, an unspaced `|` alternative values. Text after `→` is the
/// answer and is skipped. Errs where the doc strays from that grammar.
pub fn parse_params(doc: &str) -> std::result::Result<Vec<Param<'_>>, String> {
    let doc = doc.split('→').next().unwrap_or_default();
    let mut params: Vec<Param> = vec![];
    let mut open: Vec<char> = vec![];
    let mut expect_key = false;
    let mut or = false;
    let mut objects: usize = 0;
    let mut about_from: Option<usize> = None;
    let mut chars = doc.char_indices().peekable();
    let mut prev = ' ';
    fn end_about<'a>(doc: &'a str, params: &mut [Param<'a>], from: &mut Option<usize>, to: usize) {
        if let (Some(f), Some(p)) = (from.take(), params.last_mut()) {
            p.about = doc.get(f..to).unwrap_or_default().trim();
        }
    }
    while let Some((i, c)) = chars.next() {
        let in_object = open == ['{'];
        match c {
            '{' | '[' | '(' => {
                open.push(c);
                if open == ['{'] {
                    expect_key = true;
                    objects += 1;
                    or = false;
                }
            }
            '}' | ']' | ')' => {
                if in_object {
                    end_about(doc, &mut params, &mut about_from, i);
                }
                open.pop();
            }
            ',' if in_object => {
                end_about(doc, &mut params, &mut about_from, i);
                expect_key = true;
            }
            '|' if in_object && prev == ' ' && chars.peek().is_some_and(|(_, n)| *n == ' ') => {
                end_about(doc, &mut params, &mut about_from, i);
                expect_key = true;
                or = true;
            }
            c if in_object && expect_key && (c.is_ascii_alphabetic() || c == '_' || c == '…') => {
                let mut end = i + c.len_utf8();
                while let Some((j, n)) = chars.peek().copied().filter(|(_, n)| n.is_ascii_alphanumeric() || *n == '_') {
                    end = j + n.len_utf8();
                    chars.next();
                }
                let name = doc.get(i..end).unwrap_or_default();
                let after = doc.get(end..).unwrap_or_default();
                let rest = after.trim_start_matches('?').trim_start();
                let at = doc.len() - rest.len();
                about_from = match rest.chars().next() {
                    Some(':') => Some(at + 1),
                    Some('(') => Some(at),
                    Some(',' | '}' | '|') => None,
                    _ => return Err(format!("`{name}` is followed by `{}`", rest.chars().take(12).collect::<String>())),
                };
                params.push(Param { name, optional: after.starts_with('?'), about: "", or, object: objects.saturating_sub(1) });
                or = false;
                expect_key = false;
            }
            c if in_object && expect_key && !c.is_whitespace() => {
                return Err(format!("expected a key at `{}`", doc.get(i..).unwrap_or_default()));
            }
            _ => {}
        }
        prev = c;
    }
    Ok(params)
}

/// Refuse keys `spec`'s doc doesn't name. An object with `…` takes any key; one without is checked
/// whole once any of its keys is given, so `{attrs: {…}} | {size?, …}` refuses `{attrs, bogus}`.
pub(crate) fn check_params(spec: &CommandSpec, p: &Value) -> Result<()> {
    let given = match p {
        Value::Null => return Ok(()),
        Value::Object(m) => m,
        other => {
            let kind = match other {
                Value::Array(_) => "an array",
                Value::String(_) => "a string",
                Value::Number(_) => "a number",
                _ => "a boolean",
            };
            return Err(bad(spec.id, format!("parameters are a JSON object, not {kind}")));
        }
    };
    let Ok(params) = parse_params(spec.params) else { return Ok(()) };
    let objects = params.iter().map(|p| p.object).max().map_or(0, |n| n + 1);
    let keys_of = |o: usize| params.iter().filter(|p| p.object == o).map(|p| p.name).collect::<Vec<_>>();
    let unknown_in = |keys: &[&str]| given.keys().filter(|k| !keys.contains(&k.as_str())).cloned().collect::<Vec<String>>();
    let open: Vec<usize> = (0..objects).filter(|o| keys_of(*o).contains(&"…")).collect();
    let unknown = if open.is_empty() {
        unknown_in(&param_keys(spec.params).unwrap_or_default())
    } else {
        (0..objects)
            .filter(|o| !open.contains(o))
            .map(keys_of)
            .filter(|keys| given.keys().any(|k| keys.contains(&k.as_str())))
            .map(|keys| unknown_in(&keys))
            .find(|u| !u.is_empty())
            .unwrap_or_default()
    };
    if unknown.is_empty() {
        return Ok(());
    }
    Err(EngineError::UnknownParams { cmd: spec.id.into(), unknown })
}

macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: true, undoable: true }
    };
    (query $id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: false, undoable: false }
    };
    (noundo $id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: true, undoable: false }
    };
}
pub(crate) use cmd;

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: std::sync::OnceLock<Vec<CommandSpec>> = std::sync::OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(interchange::specs());
        v.extend(export::specs());
        v.extend(edit::specs());
        v.extend(endnotes::specs());
        v.extend(editorial::specs());
        v.extend(object::specs());
        v.extend(anchors::specs());
        v.extend(articles::specs());
        v.extend(paths::specs());
        v.extend(book::specs());
        v.extend(buttons::specs());
        v.extend(captions::specs());
        v.extend(colorsettings::specs());
        v.extend(changes::specs());
        v.extend(conditions::specs());
        v.extend(library::specs());
        v.extend(lists::specs());
        v.extend(linked::specs());
        v.extend(mathexpr::specs());
        v.extend(media::specs());
        v.extend(pdflayers::specs());
        v.extend(liquid::specs());
        v.extend(xml::specs());
        v.extend(strokes::specs());
        v.extend(scripts::specs());
        v.extend(states::specs());
        v.extend(transitions::specs());
        v.extend(path_type::specs());
        v.extend(qr::specs());
        v.extend(select::specs());
        v.extend(package::specs());
        v.extend(print::specs());
        v.extend(text::specs());
        v.extend(style::specs());
        v.extend(table::specs());
        v.extend(layout::specs());
        v.extend(overrides::specs());
        v.extend(prefs::specs());
        v.extend(inspect::specs());
        v.extend(find::specs());
        v.extend(variables::specs());
        v.extend(toc::specs());
        v.extend(notes::specs());
        v.extend(xref::specs());
        v.extend(index::specs());
        v.extend(anchored::specs());
        v.extend(guides::specs());
        v.extend(links::specs());
        v.extend(fonts::specs());
        v.extend(spelling::specs());
        v.extend(datamerge::specs());
        v.extend(interactive::specs());
        v.extend(preflight::specs());
        v
    })
}

pub fn find_command(id: &str) -> Option<&'static CommandSpec> {
    command_specs().iter().find(|c| c.id == id)
}

// ---------- param helpers ----------

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
/// `p` with `key` set to `v`. Parameters come from callers (control channel, MCP, scripts) and
/// may not be an object: `p[key] = v` panics on an array or a string, so start from `{}` then.
pub(crate) fn with_param(p: &Value, key: &str, v: Value) -> Value {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.insert(key.to_string(), v);
    Value::Object(m)
}
pub(crate) fn f64_or(p: &Value, key: &str, default: f64) -> f64 {
    p.get(key).and_then(Value::as_f64).unwrap_or(default)
}
pub(crate) fn bool_or(p: &Value, key: &str, default: bool) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(default)
}
pub(crate) fn str_param<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}
/// Plain text from a parameter, with CR LF and lone CR (classic Mac and InDesign line ends) as
/// `\n`, the paragraph separator, as edit.paste and text import already do. Kept as they were,
/// the CRs joined the paragraphs into one.
pub(crate) fn text_param(p: &Value, key: &str) -> String {
    str_param(p, key).unwrap_or("").replace("\r\n", "\n").replace('\r', "\n")
}
pub(crate) fn id_param(p: &Value, key: &str) -> Option<ItemId> {
    p.get(key).and_then(Value::as_u64).map(ItemId)
}
pub(crate) fn ids_param(p: &Value, key: &str) -> Option<Vec<ItemId>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ItemId).collect())
}
pub(crate) fn point_param(p: &Value, key: &str) -> Option<Point> {
    let a = p.get(key)?.as_array()?;
    Some(Point::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}
pub(crate) fn rect_param(p: &Value, key: &str) -> Option<Rect> {
    let a = p.get(key)?.as_array()?;
    let f = |i: usize| a.get(i).and_then(Value::as_f64);
    Some(Rect::new(f(0)?, f(1)?, f(2)?, f(3)?))
}
/// `{"kind":"doc","index":0}`, `0` or absent (= spread 0).
pub(crate) fn spread_param(p: &Value, key: &str) -> SpreadRef {
    match p.get(key) {
        Some(Value::Number(n)) => SpreadRef::Doc(n.as_u64().unwrap_or(0) as usize),
        Some(v) => serde_json::from_value(v.clone()).unwrap_or(SpreadRef::Doc(0)),
        None => SpreadRef::Doc(0),
    }
}

/// The objects a call names with a non-empty `ids` or an `id`, for a command whose params
/// document `ids` (it acts on [`targets`]). `None` when the call names none.
pub(crate) fn named_targets(spec: &CommandSpec, p: &Value) -> Option<Vec<ItemId>> {
    if !spec.params.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "ids") {
        return None;
    }
    // As `targets` reads them: `ids`, when given, wins over `id` (an empty list names nothing).
    if let Some(ids) = ids_param(p, "ids") {
        return (!ids.is_empty()).then_some(ids);
    }
    id_param(p, "id").map(|i| vec![i])
}

/// Targets: `ids` / `id` params or the selection.
pub(crate) fn targets(s: &Session, p: &Value) -> Result<Vec<ItemId>> {
    if let Some(ids) = ids_param(p, "ids") {
        return Ok(ids);
    }
    if let Some(id) = id_param(p, "id") {
        return Ok(vec![id]);
    }
    Ok(s.doc()?.selection.items.clone())
}

pub(crate) fn ok() -> Result<Value> {
    Ok(Value::Null)
}

pub fn file_bytes(d: &designcraft_doc::Document) -> Vec<u8> {
    file::to_bytes(d)
}
pub fn file_from(b: &[u8]) -> Result<designcraft_doc::Document> {
    file::from_bytes(b)
}
pub(crate) use datamerge::sync_placeholders;
pub(crate) use file::load_document_fonts;
pub use file::{base64_decode, base64_encode, from_bytes, to_bytes};
pub(crate) use place_text::autoflow as place_text_autoflow;

pub(crate) use crate::describe::{no_object, none_at};

pub(crate) fn no_story(id: designcraft_doc::StoryId) -> String {
    format!("no story {} in the document; document.inspect lists the stories with their ids", id.0)
}

pub(crate) fn no_spread(d: &designcraft_doc::Document, r: SpreadRef) -> String {
    match r {
        SpreadRef::Doc(i) => none_at("spread", i, d.spreads.len()),
        SpreadRef::Parent(i) => none_at("parent spread", i, d.parents.len()),
    }
}

pub(crate) fn no_page(d: &designcraft_doc::Document, i: usize) -> String {
    none_at("page", i, d.page_count())
}

pub(crate) fn no_layer(d: &designcraft_doc::Document, id: designcraft_doc::LayerId) -> String {
    let layers = d.layers.iter().map(|l| format!("{} (id {})", l.name, l.id.0)).collect::<Vec<_>>().join(", ");
    format!("no layer with id {}; the layers are {layers}", id.0)
}
