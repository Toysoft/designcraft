use std::sync::Arc;

use designcraft_doc::{Content, Document, Item, ItemId, Selection, SpreadRef};

use crate::{DocState, UiRequest};

pub(crate) fn none_at(what: &str, i: usize, n: usize) -> String {
    match n {
        0 => format!("no {what} at index {i}; there are none"),
        n => format!("no {what} at index {i}; indices run 0–{}", n - 1),
    }
}

pub(crate) fn no_object(id: ItemId) -> String {
    format!("no object {} in the document; document.inspect lists the objects with their ids", id.0)
}

fn place(doc: &Document, id: ItemId) -> String {
    if let Some(page) = doc.page_of_item(id) {
        return format!("page {}", doc.page_name(page));
    }
    match doc.find(id).map(|l| l.spread) {
        Some(SpreadRef::Doc(si)) => format!("the pasteboard of spread {si}"),
        Some(SpreadRef::Parent(pi)) => match doc.parents.get(pi).and_then(|p| p.parent.as_ref()) {
            Some(info) => format!("parent {}-{}", info.prefix, info.name),
            None => format!("parent spread {pi}"),
        },
        None => "the document".into(),
    }
}

fn kind(item: &Item) -> &'static str {
    match &item.content {
        Content::Text(_) => "text frame",
        Content::Graphic(_) => "graphic frame",
        Content::Group { .. } => "group",
        Content::Unassigned if item.shape == designcraft_doc::Shape::GraphicLine => "line",
        Content::Unassigned => "frame",
    }
}

fn object(doc: &Document, item: &Item) -> String {
    format!("{} {} on {}", kind(item), item.id.0, place(doc, item.id))
}

fn objects(doc: &Document, items: &[&Item]) -> String {
    match items {
        [one] => object(doc, one),
        many => {
            let ids = many.iter().map(|i| i.id.0.to_string()).collect::<Vec<_>>().join(", ");
            let places: Vec<String> = many.iter().map(|i| place(doc, i.id)).collect();
            match places.first() {
                Some(p) if places.iter().all(|q| q == p) => format!("objects {ids} on {p}"),
                _ => format!("objects {ids}"),
            }
        }
    }
}

fn pages(doc: &Document, indices: &[usize]) -> String {
    let names = indices.iter().map(|i| doc.page_name(*i)).collect::<Vec<_>>().join(", ");
    if indices.len() == 1 { format!("page {names}") } else { format!("pages {names}") }
}

/// What a command changed, for a caller that got no answer from it: the document's objects, pages
/// and parts that differ between `before` and `after`, then the selection, then what the command
/// asked of the app.
pub(crate) fn changes(before: Option<(u64, Arc<Document>, Selection)>, after: Option<&DocState>, requests: &[UiRequest]) -> String {
    let mut said = match (before, after) {
        (_, None) => vec!["no document is open".to_string()],
        (Some((uid, doc, sel)), Some(st)) if uid == st.uid => {
            // `edit` allocates a new document even when nothing in it differs.
            let mut said = if Arc::ptr_eq(&doc, &st.doc) || *doc == *st.doc { vec![] } else { document(&doc, &st.doc) };
            said.extend(selection(&sel, &st.selection, &st.doc));
            said
        }
        (_, Some(st)) => vec![format!("the active document is now {}", st.title())],
    };
    said.extend(requests.iter().map(request));
    if said.is_empty() { "nothing changed".into() } else { said.join("; ") }
}

fn top_level(doc: &Document) -> Vec<&Arc<Item>> {
    doc.spreads.iter().chain(&doc.parents).flat_map(|s| s.items.iter()).collect()
}

fn document(old: &Document, new: &Document) -> Vec<String> {
    let mut said = vec![];
    let page_ids = |d: &Document| d.pages().map(|(i, _, p)| (i, p.id)).collect::<Vec<_>>();
    let (old_pages, new_pages) = (page_ids(old), page_ids(new));
    let added: Vec<usize> = new_pages.iter().filter(|(_, id)| !old_pages.iter().any(|(_, o)| o == id)).map(|(i, _)| *i).collect();
    let deleted: Vec<usize> = old_pages.iter().filter(|(_, id)| !new_pages.iter().any(|(_, n)| n == id)).map(|(i, _)| *i).collect();
    if !added.is_empty() {
        said.push(format!("added {}", pages(new, &added)));
    }
    if !deleted.is_empty() {
        said.push(format!("deleted {} (as numbered before)", pages(old, &deleted)));
    }
    let kept_old: Vec<_> = old_pages.iter().map(|(_, id)| id).filter(|id| new_pages.iter().any(|(_, n)| n == *id)).collect();
    let kept_new: Vec<_> = new_pages.iter().map(|(_, id)| id).filter(|id| kept_old.contains(id)).collect();
    if kept_old != kept_new {
        said.push("reordered the pages".into());
    }
    let (old_items, new_items) = (top_level(old), top_level(new));
    let added: Vec<&Item> = new_items.iter().filter(|i| !old_items.iter().any(|o| o.id == i.id)).map(|i| &***i).collect();
    let deleted: Vec<&Item> = old_items.iter().filter(|o| !new_items.iter().any(|i| i.id == o.id)).map(|i| &***i).collect();
    let changed: Vec<&Item> =
        new_items.iter().filter(|i| old_items.iter().any(|o| o.id == i.id && !Arc::ptr_eq(o, i) && **o != ***i)).map(|i| &***i).collect();
    if !added.is_empty() {
        said.push(format!("added {}", objects(new, &added)));
    }
    if !deleted.is_empty() {
        said.push(format!("deleted {}", objects(old, &deleted)));
    }
    if !changed.is_empty() {
        said.push(format!("changed {}", objects(new, &changed)));
    }
    let kept_old: Vec<ItemId> = old_items.iter().map(|i| i.id).filter(|id| new_items.iter().any(|n| n.id == *id)).collect();
    let kept_new: Vec<ItemId> = new_items.iter().map(|i| i.id).filter(|id| kept_old.contains(id)).collect();
    if kept_old != kept_new {
        said.push("reordered the objects".into());
    }
    let frame_said = |story: &designcraft_doc::Story| story.frames.iter().any(|f| added.iter().chain(&changed).any(|i| i.id == *f));
    let stories: Vec<String> = new
        .stories
        .iter()
        .filter(|(id, s)| match old.stories.get(id) {
            Some(o) => !Arc::ptr_eq(o, s) && **o != ***s && !frame_said(s),
            None => !frame_said(s),
        })
        .map(|(id, _)| id.0.to_string())
        .collect();
    match stories.as_slice() {
        [] => {}
        [one] => said.push(format!("changed story {one}")),
        many => said.push(format!("changed stories {}", many.join(", "))),
    }
    let deleted_stories = old.stories.keys().filter(|id| !new.stories.contains_key(id)).count();
    if deleted_stories > 0 && deleted.is_empty() {
        said.push(format!("deleted {deleted_stories} stor{}", if deleted_stories == 1 { "y" } else { "ies" }));
    }
    let parts: Vec<&str> = [
        (old.title != new.title, "title"),
        (old.settings != new.settings, "settings"),
        (old.sections != new.sections, "sections"),
        (old.layers != new.layers, "layers"),
        (!Arc::ptr_eq(&old.styles, &new.styles) && old.styles != new.styles, "styles"),
        (old.swatches != new.swatches || old.color_groups != new.color_groups, "swatches"),
        (old.stroke_styles != new.stroke_styles, "stroke styles"),
        (old.conditions != new.conditions, "conditions"),
        (old.text_variables != new.text_variables, "text variables"),
        (old.hyphenation_exceptions != new.hyphenation_exceptions, "hyphenation exceptions"),
        (old.articles != new.articles, "articles"),
        (old.xml != new.xml, "XML tags"),
        (old.inks != new.inks, "inks"),
        (old.endnote_options != new.endnote_options, "endnote options"),
        (old.assets.len() != new.assets.len(), "placed assets"),
    ]
    .into_iter()
    .filter_map(|(changed, name)| changed.then_some(name))
    .collect();
    let parents_changed = old.parents.len() != new.parents.len()
        || old.parents.iter().zip(&new.parents).any(|(a, b)| !Arc::ptr_eq(a, b) && a.pages != b.pages || a.parent != b.parent);
    if parents_changed {
        said.push("changed the parent pages".into());
    }
    let same_pages = old_pages.len() == new_pages.len() && kept_old == kept_new && added.is_empty();
    let reparented: Vec<usize> = new.pages().filter(|(i, _, p)| old.page(*i).is_some_and(|o| o.parent != p.parent)).map(|(i, _, _)| i).collect();
    if same_pages && !reparented.is_empty() {
        said.push(format!("changed the parent applied to {}", pages(new, &reparented)));
    }
    let changed_pages: Vec<usize> =
        new.pages().filter(|(i, _, p)| old.page(*i).is_some_and(|o| o != *p && o.parent == p.parent)).map(|(i, _, _)| i).collect();
    if same_pages && !changed_pages.is_empty() {
        said.push(format!("changed {}", pages(new, &changed_pages)));
    }
    if !parts.is_empty() {
        said.push(format!("changed the document's {}", parts.join(", ")));
    }
    if said.is_empty() {
        said.push("changed the document".into());
    }
    said
}

fn selection(old: &Selection, new: &Selection, doc: &Document) -> Vec<String> {
    let mut said = vec![];
    if old.items != new.items {
        let items: Vec<&Item> = new.items.iter().filter_map(|id| doc.item(*id)).collect();
        said.push(if items.is_empty() { "nothing is selected".into() } else { format!("selected {}", objects(doc, &items)) });
    }
    if old.text != new.text {
        said.push(match &new.text {
            Some(t) if t.is_caret() => format!("the caret is in story {} at byte {}", t.story.0, t.focus),
            Some(t) => {
                let text = doc.story(t.story).and_then(|s| s.text.get(t.range())).unwrap_or_default();
                let shown: String = text.chars().take(40).collect();
                let shown = if shown.len() < text.len() { format!("{shown}…") } else { shown };
                format!("selected bytes {}–{} of story {}, “{}”", t.range().start, t.range().end, t.story.0, shown.replace('\n', "¶"))
            }
            None => "no text is selected".into(),
        });
    }
    if old.cells != new.cells {
        said.push(match &new.cells {
            Some(c) => format!("selected cells [{}, {}] to [{}, {}] of table {}", c.range.r0, c.range.c0, c.range.r1, c.range.c1, c.table),
            None => "no table cells are selected".into(),
        });
    }
    said
}

fn request(r: &UiRequest) -> String {
    match r {
        UiRequest::Dialog { id, .. } => format!("asked the app to open the {id} dialog"),
        UiRequest::View { .. } => "asked the app to change the view".into(),
        UiRequest::Pick { purpose, .. } => format!("asked the app to pick a file ({purpose})"),
    }
}
