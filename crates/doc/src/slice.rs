//! Formatted text slices: copy a range of a story (with paragraph and character formats,
//! footnotes, endnotes, editorial notes, cross-references, index markers and tables) and insert
//! it into another story.

use std::sync::Arc;

use crate::endnotes::{ENDNOTE_REF, NOTE_MARK};
use crate::ids::StoryId;
use crate::index::INDEX_MARK;
use crate::notes::FOOTNOTE_REF;
use crate::story::{CharRun, Story, TABLE_ANCHOR};
use crate::xref::{ANCHOR_MARK, XREF_MARK};

impl Story {
    /// A frameless story holding `range` with its formatting and the objects anchored in it.
    pub fn extract(&self, range: std::ops::Range<usize>) -> Story {
        let a = crate::story::floor_char_boundary(&self.text, range.start.min(self.len()));
        let b = crate::story::floor_char_boundary(&self.text, range.end.min(self.len())).max(a);
        let mut out = Story::new(StoryId(0));
        out.direction = self.direction;
        out.text = self.text[a..b].to_string();
        let (p0, p1) = (self.para_at(a), self.para_at(b));
        out.paras = self.paras[p0..=p1].to_vec();
        out.chars = Vec::new();
        for (r, f) in self.runs() {
            let s = r.start.max(a);
            let e = r.end.min(b);
            if s < e {
                out.chars.push(CharRun { len: e - s, format: f.clone() });
            }
        }
        if out.chars.is_empty() {
            out.chars.push(CharRun { len: 0, format: self.char_format_at(a).clone() });
        }
        // Payloads in text order; a file may hold fewer payloads than markers, so the lists are
        // read with `get` and the slice's own lists are made to match its markers below.
        let span = |m: char| {
            let lo = self.text[..a].matches(m).count();
            lo..lo + self.text[a..b].matches(m).count()
        };
        out.notes = marked(&self.notes, span(FOOTNOTE_REF));
        out.endnotes = marked(&self.endnotes, span(ENDNOTE_REF));
        out.editorial = marked(&self.editorial, span(NOTE_MARK));
        out.anchors = marked(&self.anchors, span(ANCHOR_MARK));
        out.xrefs = marked(&self.xrefs, span(XREF_MARK));
        out.index_refs = marked(&self.index_refs, span(INDEX_MARK));
        out.objects = marked(&self.objects, span(crate::anchored::OBJECT_MARK));
        out.fix_notes();
        out.fix_endnotes();
        out.fix_marks();
        for p in &out.paras {
            if let Some(id) = p.table
                && let Some(t) = self.tables.get(&id)
                && out.text.contains(TABLE_ANCHOR)
            {
                out.tables.insert(id, t.clone());
            }
        }
        out.fix_tables();
        out
    }

    /// Insert a slice made by [`Story::extract`] at `pos`, keeping its formatting: the first
    /// pasted paragraph merges into the paragraph at `pos` (it keeps that paragraph's format, as
    /// in InDesign), later paragraphs keep their own formats. Returns the end of the insertion.
    pub fn insert_story(&mut self, pos: usize, src: &Story) -> usize {
        let pos = crate::story::floor_char_boundary(&self.text, pos.min(self.len()));
        if src.text.is_empty() {
            return pos;
        }
        // Text first (creates empty marker entries), then formats and marker payloads.
        let mut at = pos;
        for (r, f) in src.runs() {
            if r.is_empty() {
                continue;
            }
            self.insert_with(at, &src.text[r.clone()], f.clone());
            at += r.len();
        }
        let end = at;
        // Paragraph formats of the pasted paragraphs after the first.
        let p0 = self.para_at(pos);
        for (k, pf) in src.paras.iter().enumerate().skip(1) {
            if let Some(p) = self.paras.get_mut(p0 + k) {
                *p = crate::story::ParaFormat { table: None, ..pf.clone() };
            }
        }
        // Payloads of notes, cross-references and index markers, in order.
        let first = |m: char| self.text[..pos].matches(m).count();
        let (nf, nx, ni, na) = (first(FOOTNOTE_REF), first(XREF_MARK), first(INDEX_MARK), first(ANCHOR_MARK));
        let (no, ne, nn) = (first(crate::anchored::OBJECT_MARK), first(ENDNOTE_REF), first(NOTE_MARK));
        for (k, o) in src.objects.iter().enumerate() {
            if let Some(slot) = self.objects.get_mut(no + k) {
                *slot = o.clone();
            }
        }
        for (k, n) in src.notes.iter().enumerate() {
            if let Some(slot) = self.notes.get_mut(nf + k) {
                let id = slot.id;
                *slot = Arc::new(crate::notes::Footnote { id, text: n.text.clone() });
            }
        }
        // Notes keep the ids made for their new markers, so a pasted copy never shares an id.
        for (k, n) in src.endnotes.iter().enumerate() {
            if let Some(slot) = self.endnotes.get_mut(ne + k) {
                let id = slot.id;
                *slot = Arc::new(crate::notes::Footnote { id, text: n.text.clone() });
            }
        }
        for (k, n) in src.editorial.iter().enumerate() {
            if let Some(slot) = self.editorial.get_mut(nn + k) {
                let id = slot.id;
                *slot = Arc::new(crate::endnotes::EditorialNote { id, ..(**n).clone() });
            }
        }
        for (k, x) in src.xrefs.iter().enumerate() {
            if let Some(slot) = self.xrefs.get_mut(nx + k) {
                *slot = x.clone();
            }
        }
        for (k, r) in src.index_refs.iter().enumerate() {
            if let Some(slot) = self.index_refs.get_mut(ni + k) {
                *slot = r.clone();
            }
        }
        // Anchors are new destinations (fresh ids) with the copied names.
        for (k, a) in src.anchors.iter().enumerate() {
            if let Some(slot) = self.anchors.get_mut(na + k) {
                let id = slot.id;
                *slot = Arc::new(crate::xref::TextAnchor { id, name: a.name.clone() });
            }
        }
        // Tables: re-attach by paragraph (fresh ids so a pasted copy never shares an id).
        if !src.tables.is_empty() {
            let mut next = self.tables.keys().max().copied().unwrap_or(0) + 1;
            for (k, pf) in src.paras.iter().enumerate() {
                let Some(tid) = pf.table else { continue };
                let Some(t) = src.tables.get(&tid) else { continue };
                let pi = p0 + k;
                let mut t = (**t).clone();
                t.id = next;
                next += 1;
                self.tables.insert(t.id, Arc::new(t));
                if let Some(p) = self.paras.get_mut(pi) {
                    p.table = Some(next - 1);
                }
            }
            self.fix_tables();
        }
        self.rev += 1;
        end
    }
}

/// The payloads of the markers numbered `span` (in text order), as far as `list` holds them.
fn marked<T: Clone>(list: &[T], span: std::ops::Range<usize>) -> Vec<T> {
    let n = list.len();
    list.get(span.start.min(n)..span.end.min(n)).map(<[T]>::to_vec).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::ParaFormat;

    #[test]
    fn copy_paste_keeps_formatting_and_notes() {
        let mut a = Story::with_text(StoryId(1), "Alpha beta\nGamma delta", ParaFormat::default());
        a.format_chars(0..5, |f| f.over.size = Some(20.0));
        a.format_paras(11..11, |p| p.style = "Head".into());
        a.insert_note(8, "A note.", ParaFormat::default());
        a.check().unwrap();
        let slice = a.extract(0..a.len());
        slice.check().unwrap();
        assert_eq!(slice.notes.len(), 1);
        let mut b = Story::with_text(StoryId(2), "Start: end", ParaFormat::default());
        let end = b.insert_story(7, &slice);
        b.check().unwrap();
        assert_eq!(&b.text[7..end], slice.text.as_str());
        assert_eq!(b.char_format_at(8).over.size, Some(20.0));
        assert_eq!(b.paras[1].style, "Head");
        assert_eq!(b.notes.len(), 1);
        assert_eq!(b.notes[0].text.text, "A note.");
        // A partial slice.
        let e = 8 + FOOTNOTE_REF.len_utf8();
        let part = a.extract(2..e);
        part.check().unwrap();
        assert_eq!(part.text, a.text[2..e]);
        assert_eq!(part.notes.len(), 1);
        assert!(a.extract(2..8).notes.is_empty());
    }

    #[test]
    fn copy_paste_keeps_endnotes_and_editorial_notes() {
        let mut a = Story::with_text(StoryId(1), "One two three", ParaFormat::default());
        a.insert_endnote(3, "An endnote.", ParaFormat::default());
        let at = a.text.find("three").unwrap();
        a.insert(at, &NOTE_MARK.to_string());
        let n = Arc::make_mut(&mut a.editorial[0]);
        (n.author, n.text) = ("Ed".into(), "Check this.".into());
        a.check().unwrap();
        let slice = a.extract(0..a.len());
        slice.check().unwrap();
        assert_eq!(slice.endnotes[0].text.text, "An endnote.");
        assert_eq!(slice.editorial[0].text, "Check this.");
        // Duplicate into the same story: the copies get their own ids and are independent.
        let len = a.len();
        a.insert_story(len, &slice);
        a.check().unwrap();
        assert_eq!(a.endnotes.len(), 2);
        assert_eq!(a.editorial.len(), 2);
        assert_ne!(a.endnotes[0].id, a.endnotes[1].id);
        assert_ne!(a.editorial[0].id, a.editorial[1].id);
        assert_eq!(a.endnotes[1].text.text, "An endnote.");
        assert_eq!((a.editorial[1].author.as_str(), a.editorial[1].text.as_str()), ("Ed", "Check this."));
        let copy = a.endnotes[1].id;
        a.endnote_mut(copy).unwrap().text = Story::with_text(StoryId(0), "Changed.", ParaFormat::default());
        Arc::make_mut(&mut a.editorial[1]).text = "Changed.".into();
        assert_eq!(a.endnotes[0].text.text, "An endnote.");
        assert_eq!(a.editorial[0].text, "Check this.");
        assert_eq!(slice.endnotes[0].text.text, "An endnote.");
        // Into another story, between existing notes.
        let mut b = Story::with_text(StoryId(2), "x y", ParaFormat::default());
        b.insert_endnote(0, "B first.", ParaFormat::default());
        b.insert_endnote(b.len(), "B last.", ParaFormat::default());
        let mid = b.text.find(' ').unwrap();
        b.insert_story(mid, &slice);
        b.check().unwrap();
        let texts: Vec<_> = b.endnotes.iter().map(|n| n.text.text.as_str()).collect();
        assert_eq!(texts, ["B first.", "An endnote.", "B last."]);
        assert_eq!(b.editorial[0].text, "Check this.");
    }

    #[test]
    fn extract_survives_more_markers_than_payloads() {
        let mut a = Story::with_text(StoryId(1), "a b c d e f", ParaFormat::default());
        for (pos, m) in [(1, FOOTNOTE_REF), (4, ENDNOTE_REF), (7, NOTE_MARK), (10, XREF_MARK)] {
            a.insert(pos, &format!("{m}{m}"));
        }
        // A hostile file: two markers of each kind, at most one payload.
        a.notes.truncate(1);
        a.endnotes.truncate(1);
        a.editorial.clear();
        a.xrefs.clear();
        for r in [0..a.len(), 2..a.len(), 0..3] {
            let slice = a.extract(r);
            slice.check().unwrap();
            let mut b = Story::with_text(StoryId(2), "z", ParaFormat::default());
            b.insert_story(1, &slice);
            b.check().unwrap();
        }
    }
}
