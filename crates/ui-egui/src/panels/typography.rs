//! The Typography panel: the OpenType features the fonts of the selected text offer, grouped as
//! Affinity's are. A feature is listed only when a font in the selection has it (the features of
//! several fonts are merged); changes apply to the selection through `type.char`.

use std::collections::BTreeSet;

use designcraft_doc::otf;
use serde_json::{Value, json};

use crate::DesignApp;

/// A switch: its OpenType tag and menu label.
type Switch = (&'static str, &'static str);

/// One choice of a radio group: its label and the tags it turns on (none for the default).
type Choice = (&'static str, &'static [&'static str]);

const LIGATURES: &[Switch] =
    &[("liga", "Standard Ligatures"), ("dlig", "Discretionary Ligatures"), ("hlig", "Historical Ligatures"), ("clig", "Contextual Ligatures")];

const ALTERNATES: &[Switch] = &[
    ("calt", "Contextual Alternates"),
    ("salt", "Stylistic Alternates"),
    ("swsh", "Swash"),
    ("titl", "Titling Alternates"),
    ("hist", "Historical Forms"),
    ("ornm", "Ornaments"),
    ("zero", "Slashed Zero"),
];

const FIGURE_STYLE: &[Choice] = &[("Default", &[]), ("Lining", &["lnum"]), ("Oldstyle", &["onum"])];

const FIGURE_WIDTH: &[Choice] = &[("Default", &[]), ("Tabular", &["tnum"]), ("Proportional", &["pnum"])];

const FIGURE_POSITION: &[Choice] = &[
    ("Normal", &[]),
    ("Numerator", &["numr"]),
    ("Denominator", &["dnom"]),
    ("Superscript", &["sups"]),
    ("Subscript", &["subs"]),
    ("Scientific Inferiors", &["sinf"]),
];

const FRACTION_AND_ORDINAL: &[Switch] = &[("frac", "Fractions"), ("ordn", "Ordinal")];

const CAPITALS: &[Choice] = &[("Normal", &[]), ("Caps to Small Caps", &["c2sc"]), ("Small Caps", &["smcp"]), ("All Small Caps", &["c2sc", "smcp"])];

/// The features the selected fonts offer: their tags, and the stylistic sets (number, name).
struct Offered {
    tags: BTreeSet<String>,
    sets: Vec<(u32, String)>,
}

impl Offered {
    fn has(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }

    /// A choice is offered when every tag it turns on is (the default always is).
    fn offers(&self, tags: &[&str]) -> bool {
        tags.iter().all(|t| self.has(t))
    }
}

pub fn show(app: &mut DesignApp, ui: &mut egui::Ui) {
    let Some(attrs) = super::text_attrs(app) else {
        crate::rtl::label(ui, crate::i18n::tr(&app.ui.language, "Select text or a text frame."));
        return;
    };
    let applied: Vec<String> =
        attrs["chars"]["otfFeatures"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let offered = offered(app);
    if offered.tags.is_empty() && offered.sets.is_empty() {
        crate::rtl::label(ui, crate::i18n::tr(&app.ui.language, "The selected fonts have no OpenType features."));
        return;
    }
    let lang = app.ui.language.clone();
    let mut list = applied.clone();
    let mut changed = false;
    crate::rtl::label(ui, egui::RichText::new(crate::i18n::tr(&lang, "Typography")).strong());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut section = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui, &mut Vec<String>) -> bool| {
            egui::CollapsingHeader::new(crate::i18n::tr(&lang, title)).id_salt(title).default_open(true).show(ui, |ui| {
                changed |= body(ui, &mut list);
            });
        };
        section(ui, "Ligatures", &mut |ui, list| switches(ui, &lang, list, &offered, LIGATURES));
        section(ui, "Alternates", &mut |ui, list| switches(ui, &lang, list, &offered, ALTERNATES));
        section(ui, "Stylistic Sets", &mut |ui, list| stylistic_sets(ui, list, &offered));
        section(ui, "Figure Style", &mut |ui, list| choices(ui, &lang, list, &offered, FIGURE_STYLE));
        section(ui, "Figure Width", &mut |ui, list| choices(ui, &lang, list, &offered, FIGURE_WIDTH));
        section(ui, "Figure Position", &mut |ui, list| {
            let a = choices(ui, &lang, list, &offered, FIGURE_POSITION);
            let b = switches(ui, &lang, list, &offered, FRACTION_AND_ORDINAL);
            a | b
        });
        section(ui, "Capitals", &mut |ui, list| choices(ui, &lang, list, &offered, CAPITALS));
    });
    if changed {
        let _ = app.run("type.char", json!({"attrs": {"otfFeatures": list}}));
    }
}

/// The features the fonts of the selection offer, merged over them.
fn offered(app: &mut DesignApp) -> Offered {
    let v = app.session.execute("type.openTypeFeatures", &json!({})).unwrap_or(Value::Null);
    let tags = v["features"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let sets = v["stylisticSets"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| Some((u32::try_from(s["set"].as_u64()?).ok()?, s["name"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Offered { tags, sets }
}

/// On/off switches the fonts offer; returns whether one changed.
fn switches(ui: &mut egui::Ui, lang: &str, list: &mut Vec<String>, offered: &Offered, items: &[Switch]) -> bool {
    let mut changed = false;
    for (tag, label) in items.iter().filter(|(tag, _)| offered.has(tag)) {
        let mut on = otf::is_on(list, tag);
        if ui.checkbox(&mut on, crate::rtl::widget(ui, crate::i18n::tr(lang, label))).changed() {
            otf::set(list, tag, on);
            changed = true;
        }
    }
    changed
}

/// Radio choices (one of several) for a group of tags; returns whether the choice changed.
fn choices(ui: &mut egui::Ui, lang: &str, list: &mut Vec<String>, offered: &Offered, items: &[Choice]) -> bool {
    let offered_items: Vec<&Choice> = items.iter().filter(|(_, tags)| offered.offers(tags)).collect();
    // Nothing to choose between unless a choice besides the default is offered.
    if offered_items.len() < 2 {
        return false;
    }
    let current =
        offered_items.iter().position(|(_, tags)| tags.iter().all(|t| list.iter().any(|x| x == t)) && (!tags.is_empty() || !group_on(list, items)));
    let mut picked = None;
    for (i, (label, _)) in offered_items.iter().enumerate() {
        if ui.radio(current == Some(i), crate::rtl::widget(ui, crate::i18n::tr(lang, label))).clicked() {
            picked = Some(i);
        }
    }
    let Some(i) = picked else { return false };
    // Only the group's own tags change: the chosen tags go on, the rest of the group goes off.
    for (_, tags) in items {
        for t in *tags {
            list.retain(|x| x != t);
        }
    }
    if let Some((_, tags)) = offered_items.get(i) {
        list.extend(tags.iter().map(|t| t.to_string()));
    }
    true
}

/// Whether any tag of a radio group is on (so the default choice is not).
fn group_on(list: &[String], items: &[Choice]) -> bool {
    items.iter().flat_map(|(_, tags)| tags.iter()).any(|t| list.iter().any(|x| x == t))
}

/// Stylistic sets, by the names their fonts give them; returns whether one changed.
fn stylistic_sets(ui: &mut egui::Ui, list: &mut Vec<String>, offered: &Offered) -> bool {
    let mut changed = false;
    for (n, name) in &offered.sets {
        let tag = format!("ss{n:02}");
        let mut on = otf::is_on(list, &tag);
        if ui.checkbox(&mut on, crate::rtl::widget(ui, format!("{name} ({tag})"))).changed() {
            otf::set(list, &tag, on);
            changed = true;
        }
    }
    changed
}
