//! Fit Frame to Content for text frames: a frame bigger than its text shrinks around it. It never
//! grows: an overset frame keeps its size rather than stretching down a long story.
//!
//! The fitted size is found by composing the story at trial sizes, so whatever sets the lines
//! (alignment, indents, tabs, drop caps, wraps, columns) is accounted for:
//! - the height is the smallest that keeps the frame's text in it, when the frame holds the end of
//!   its story: text that runs on (overset, or into the next frame) keeps the frame's height;
//! - the width shrinks only when no line reaches across the frame: every line is ended by its
//!   paragraph or a break, none wraps. It is then the narrowest that keeps every line as it is.
//!   Running text that wraps keeps the frame's width.
//!
//! The side kept in place follows the text, so it doesn't move on the page: the left, right or
//! centre (whichever the text sits against) and, vertically, the frame's vertical justification.

use designcraft_compose::{self as compose, ComposeOptions, ComposedStory, FrameSpec, FrameText};
use designcraft_doc::{Document, ItemId, Story, VerticalJustification};
use designcraft_geom::Rect;

use crate::Session;

/// Trial sizes stop once the bounds are this close (points).
const PRECISION: f64 = 0.01;
/// The narrowest text area tried (points).
const MIN_SIZE: f64 = 0.5;

/// Which of a text frame's sides fitting may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Axes {
    pub width: bool,
    pub height: bool,
}

impl Axes {
    /// `both` (a corner handle), `width` (a side handle) or `height` (a top or bottom handle).
    pub(crate) fn parse(s: &str) -> Option<Axes> {
        match s {
            "both" => Some(Axes { width: true, height: true }),
            "width" => Some(Axes { width: true, height: false }),
            "height" => Some(Axes { width: false, height: true }),
            _ => None,
        }
    }
}

/// The smaller inner bounds (item space) text frame `id` takes to fit its text, or `None` when it
/// can't shrink: not a text frame, type on a path, vertical text (not supported yet), no text in
/// the frame, its text fills it already, or every line that could narrow it wraps.
pub(crate) fn fitted_bounds(s: &Session, doc: &Document, id: ItemId, axes: Axes) -> Option<Rect> {
    let item = doc.item(id)?;
    let tf = item.text_frame()?;
    if tf.options.path.is_some() || doc.frame_vertical(item) {
        return None;
    }
    let story = doc.story(tf.story)?;
    let mut specs = compose::frame_specs(doc, tf.story);
    let k = specs.iter().position(|f| f.id == id)?;
    // Frames after this one don't change what this one holds.
    specs.truncate(k + 1);
    let opts = ComposeOptions { running: Some(s.cache.running_index(doc)), xrefs: Some(s.cache.xref_index(doc)), ..Default::default() };
    let fit = Fit { doc, story, opts, k };
    let area = specs.get(k)?.area;
    let inner = item.inner_bounds();
    // Insets (and an inside stroke) stay as they are around the text area.
    let pads = [area.y0 - inner.y0, area.x0 - inner.x0, inner.y1 - area.y1, inner.x1 - area.x1];

    // The frame's text as it is now: what fitting keeps.
    let cs = fit.compose(&mut specs, area)?;
    let ft = cs.frame(id)?;
    if !ft.lines.iter().any(|l| l.glyphs.iter().any(|g| g.visible)) {
        return None;
    }
    let lines = line_ranges(ft);
    let end = ft.range.end;
    // The text runs on past the frame (overset, or into the next frame of its thread): the frame
    // isn't bigger than its text.
    let runs_on = cs.overset_at.is_some();
    let narrowable = axes.width && ft.tables.is_empty() && ft.lines.iter().all(|l| ends_by_itself(story, l));
    let mut new = area;
    if narrowable {
        let side = fit.horizontal_side(ft, area);
        let w = smallest(MIN_SIZE, area.width(), |w| fit.lines(&mut specs, side.place_x(area, w)).is_some_and(|l| l == lines))?;
        // Within the search's precision of the width it has, the frame is fitted already.
        if area.width() - w > PRECISION {
            new = side.place_x(new, w);
        }
    }
    if axes.height && !runs_on {
        let side = match specs.get(k)?.opts.vertical_justification {
            VerticalJustification::Top | VerticalJustification::Justify => Side::Start,
            VerticalJustification::Center => Side::Center,
            VerticalJustification::Bottom => Side::End,
        };
        let holds = |specs: &mut Vec<FrameSpec>, h: f64| fit.holds(specs, side.place_y(new, h), end);
        // Never taller than it is: a frame that can't hold its text at its own height keeps it.
        if holds(&mut specs, area.height()) {
            let h = smallest(MIN_SIZE, area.height(), |h| holds(&mut specs, h))?;
            if area.height() - h > PRECISION {
                new = side.place_y(new, h);
            }
        }
    }
    let fitted = Rect::new(new.x0 - pads[1], new.y0 - pads[0], new.x1 + pads[3], new.y1 + pads[2]);
    let same = (fitted.x0 - inner.x0).abs() < 1e-6
        && (fitted.y0 - inner.y0).abs() < 1e-6
        && (fitted.x1 - inner.x1).abs() < 1e-6
        && (fitted.y1 - inner.y1).abs() < 1e-6;
    (!same && fitted.width() > 0.0 && fitted.height() > 0.0).then_some(fitted)
}

/// The frame's lines as story byte ranges.
fn line_ranges(ft: &FrameText) -> Vec<(usize, usize)> {
    ft.lines.iter().map(|l| (l.range.start, l.range.end)).collect()
}

/// Is the line ended by its paragraph or by a break (a forced line, column, frame or page break),
/// rather than wrapped at the frame's edge?
fn ends_by_itself(story: &Story, line: &compose::Line) -> bool {
    line.last_in_para || story.text.get(..line.range.end).and_then(|t| t.chars().next_back()).is_some_and(compose::breaker::is_forced)
}

/// The smallest value in `lo..=hi` for which `ok` holds, given that it holds for `hi` and keeps
/// holding above any value it holds for.
fn smallest(lo: f64, hi: f64, mut ok: impl FnMut(f64) -> bool) -> Option<f64> {
    if !(lo.is_finite() && hi.is_finite()) || hi < lo {
        return None;
    }
    if ok(lo) {
        return Some(lo);
    }
    let (mut lo, mut hi) = (lo, hi);
    // Bounded: the interval halves each time.
    for _ in 0..64 {
        if hi - lo <= PRECISION {
            break;
        }
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// The side of the text area kept in place.
#[derive(Clone, Copy)]
enum Side {
    Start,
    Center,
    End,
}

impl Side {
    fn place_x(self, r: Rect, w: f64) -> Rect {
        let x0 = match self {
            Side::Start => r.x0,
            Side::Center => r.center().x - w / 2.0,
            Side::End => r.x1 - w,
        };
        Rect::new(x0, r.y0, x0 + w, r.y1)
    }

    fn place_y(self, r: Rect, h: f64) -> Rect {
        let y0 = match self {
            Side::Start => r.y0,
            Side::Center => r.center().y - h / 2.0,
            Side::End => r.y1 - h,
        };
        Rect::new(r.x0, y0, r.x1, y0 + h)
    }
}

/// One frame of a story being fitted: compositions of the story through it.
struct Fit<'a> {
    doc: &'a Document,
    story: &'a Story,
    opts: ComposeOptions,
    /// The frame's index in the story's frames.
    k: usize,
}

impl Fit<'_> {
    fn compose(&self, specs: &mut [FrameSpec], area: Rect) -> Option<ComposedStory> {
        specs.get_mut(self.k)?.area = area;
        Some(compose::compose(self.doc, self.story, specs, &self.opts))
    }

    /// The frame's lines (story byte ranges) with its text area at `area`.
    fn lines(&self, specs: &mut [FrameSpec], area: Rect) -> Option<Vec<(usize, usize)>> {
        let cs = self.compose(specs, area)?;
        Some(line_ranges(cs.frame(specs.get(self.k)?.id)?))
    }

    /// Does the frame, with its text area at `area`, still hold its text up to story byte `end`?
    fn holds(&self, specs: &mut [FrameSpec], area: Rect, end: usize) -> bool {
        let Some(cs) = self.compose(specs, area) else { return false };
        specs.get(self.k).and_then(|f| cs.frame(f.id)).is_some_and(|ft| ft.range.end >= end)
    }

    /// The side the text sits against in `ft` (composed in `area`): the one with less room between
    /// the text and the area's edge, or the centre when the room is the same on both sides.
    fn horizontal_side(&self, ft: &FrameText, area: Rect) -> Side {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for g in ft.lines.iter().flat_map(|l| l.glyphs.iter()).filter(|g| g.visible) {
            lo = lo.min(g.x.min(g.x + g.adv));
            hi = hi.max(g.x.max(g.x + g.adv));
        }
        if !(lo.is_finite() && hi.is_finite()) {
            return Side::Start;
        }
        let (left, right) = (lo - area.x0, area.x1 - hi);
        if left > 0.5 && (left - right).abs() <= 0.5 {
            Side::Center
        } else if right < left {
            Side::End
        } else {
            Side::Start
        }
    }
}

#[cfg(test)]
mod tests {
    use designcraft_doc::{ItemId, StoryId};
    use serde_json::{Value, json};

    use crate::Session;

    const LONG: &str = "The quick brown fox jumps over the lazy dog and keeps on running through the field until the sun goes down.";

    /// A new document with a text frame at [72, 72, 400, 200] holding `text`; returns its id.
    fn frame(s: &mut Session, text: &str) -> u64 {
        s.execute("file.new", &json!({})).unwrap();
        text_frame(s, [72.0, 72.0, 400.0, 200.0], text)
    }

    fn text_frame(s: &mut Session, rect: [f64; 4], text: &str) -> u64 {
        s.execute("frame.create", &json!({"rect": rect, "content": "text", "text": text})).unwrap()["id"].as_u64().unwrap()
    }

    fn fit(s: &mut Session, id: u64, axes: &str) -> Value {
        s.execute("object.fit", &json!({"mode": "fitFrameToContent", "axes": axes, "ids": [id]})).unwrap()
    }

    /// The frame's inner bounds (its own space).
    fn inner(s: &Session, id: u64) -> (f64, f64, f64, f64) {
        let b = s.doc().unwrap().doc.item(ItemId(id)).unwrap().inner_bounds();
        (b.x0, b.y0, b.x1, b.y1)
    }

    fn story(s: &Session, id: u64) -> StoryId {
        s.doc().unwrap().doc.item(ItemId(id)).unwrap().text_frame().unwrap().story
    }

    /// The frame's lines as story byte ranges, and whether the story is overset.
    fn lines(s: &Session, id: u64) -> (Vec<(usize, usize)>, bool) {
        let d = s.doc().unwrap();
        let cs = s.cache.get(&d.doc, story(s, id), None);
        let ft = cs.frame(ItemId(id)).unwrap();
        (ft.lines.iter().map(|l| (l.range.start, l.range.end)).collect(), cs.is_overset())
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn a_corner_fit_shrinks_around_one_line_from_the_top_left() {
        let mut s = Session::new();
        let id = frame(&mut s, "Hi there");
        let (x0, y0, x1, y1) = inner(&s, id);
        let before = lines(&s, id);
        assert_eq!(fit(&mut s, id, "both")["fitted"], 1);
        let (a, b, c, d) = inner(&s, id);
        assert!(close(a, x0) && close(b, y0), "top left kept: {a}, {b}");
        assert!(c < x1 && d < y1, "shrunk: {c}, {d}");
        assert_eq!(lines(&s, id), before, "same line, nothing overset");
        // Fitted already: a second fit changes nothing.
        assert_eq!(fit(&mut s, id, "both")["fitted"], 0);
    }

    #[test]
    fn a_side_handle_fits_only_the_width_and_a_top_handle_only_the_height() {
        let mut s = Session::new();
        let id = frame(&mut s, "Hi there");
        let (x0, y0, x1, y1) = inner(&s, id);
        fit(&mut s, id, "height");
        let (a, b, c, d) = inner(&s, id);
        assert!(close(a, x0) && close(c, x1), "width kept");
        assert!(close(b, y0) && d < y1, "height fitted");
        fit(&mut s, id, "width");
        let (a2, b2, c2, d2) = inner(&s, id);
        assert!(close(a2, x0) && c2 < x1, "width fitted");
        assert!(close(b2, b) && close(d2, d), "height kept");
    }

    #[test]
    fn running_text_that_wraps_keeps_the_width_and_shrinks_in_height() {
        let mut s = Session::new();
        let id = frame(&mut s, LONG);
        let before = lines(&s, id);
        assert!(before.0.len() > 1, "wraps: {before:?}");
        let (x0, y0, x1, y1) = inner(&s, id);
        fit(&mut s, id, "width");
        assert_eq!(inner(&s, id), (x0, y0, x1, y1), "a side handle changes nothing");
        fit(&mut s, id, "both");
        let (a, b, c, d) = inner(&s, id);
        assert!(close(a, x0) && close(c, x1), "width kept");
        assert!(close(b, y0) && d < y1, "height fitted");
        assert_eq!(lines(&s, id), before, "the same lines, nothing overset");
    }

    #[test]
    fn short_lines_ended_by_paragraphs_or_breaks_narrow_to_the_longest() {
        for text in ["First line\nA longer second line\nThird", "First line\u{2028}A longer second line\u{2028}Third"] {
            let mut s = Session::new();
            let id = frame(&mut s, text);
            let before = lines(&s, id);
            assert_eq!(before.0.len(), 3, "{text:?}");
            let (x0, _, x1, _) = inner(&s, id);
            fit(&mut s, id, "width");
            let (a, b, c, d) = inner(&s, id);
            assert!(close(a, x0) && c < x1, "{text:?}: narrower");
            assert_eq!(lines(&s, id), before, "{text:?}: the same lines");
            // As narrow as it can be: any narrower and the longest line wraps.
            let doc = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc);
            doc.item_mut(ItemId(id)).unwrap().path = designcraft_geom::shapes::rectangle(designcraft_geom::Rect::new(a, b, c - 0.5, d));
            assert_ne!(lines(&s, id).0, before.0, "{text:?}: half a point narrower wraps");
        }
    }

    #[test]
    fn a_line_that_wraps_among_short_ones_keeps_the_width() {
        let mut s = Session::new();
        let id = frame(&mut s, &format!("Short\n{LONG}"));
        let before = inner(&s, id);
        fit(&mut s, id, "width");
        assert_eq!(inner(&s, id), before);
    }

    #[test]
    fn right_aligned_and_centred_text_stay_in_place() {
        for (align, keep) in [("right", "right"), ("center", "center")] {
            let mut s = Session::new();
            let id = frame(&mut s, "Hi there");
            s.execute("text.select", &json!({"story": story(&s, id).0, "anchor": 0, "focus": 8})).unwrap();
            s.execute("type.para", &json!({"attrs": {"align": align}})).unwrap();
            let (x0, _, x1, _) = inner(&s, id);
            fit(&mut s, id, "width");
            let (a, _, c, _) = inner(&s, id);
            assert!(c - a < x1 - x0, "{align}: narrower");
            match keep {
                "right" => assert!(close(c, x1), "right edge kept: {c} vs {x1}"),
                _ => assert!(close((a + c) / 2.0, (x0 + x1) / 2.0), "centre kept"),
            }
        }
    }

    #[test]
    fn an_overset_frame_never_grows() {
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let id = text_frame(&mut s, [72.0, 72.0, 200.0, 90.0], LONG);
        let before = lines(&s, id);
        assert!(before.1, "overset to start with");
        let bounds = inner(&s, id);
        assert_eq!(fit(&mut s, id, "both")["fitted"], 0);
        assert_eq!(inner(&s, id), bounds);
        assert_eq!(lines(&s, id), before);
    }

    #[test]
    fn a_frame_whose_text_runs_into_the_next_keeps_its_size_and_the_last_one_shrinks() {
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let a = text_frame(&mut s, [72.0, 72.0, 200.0, 120.0], &format!("{LONG} {LONG}"));
        let b = text_frame(&mut s, [72.0, 200.0, 200.0, 700.0], "");
        s.execute("text.thread", &json!({"from": a, "to": b})).unwrap();
        let (before_a, before_b) = (inner(&s, a), lines(&s, b));
        assert_eq!(fit(&mut s, a, "both")["fitted"], 0);
        assert_eq!(inner(&s, a), before_a);
        // The last frame holds the story's end: it shrinks around it.
        let (_, y0, _, y1) = inner(&s, b);
        fit(&mut s, b, "height");
        let (_, b0, _, b1) = inner(&s, b);
        assert!(close(b0, y0) && b1 < y1, "shrunk: {b1} from {y1}");
        assert_eq!(lines(&s, b), before_b, "the same text, nothing overset");
    }

    #[test]
    fn a_rotated_frame_fits_in_its_own_space() {
        let mut s = Session::new();
        let id = frame(&mut s, "Hi there");
        s.execute("transform.rotate", &json!({"ids": [id], "angle": 30})).unwrap();
        let xf = s.doc().unwrap().doc.item(ItemId(id)).unwrap().xf;
        let before = lines(&s, id);
        let (x0, y0, x1, y1) = inner(&s, id);
        fit(&mut s, id, "both");
        let it = s.doc().unwrap().doc.item(ItemId(id)).unwrap();
        assert_eq!(it.xf, xf, "still turned the same way");
        let (a, b, c, d) = inner(&s, id);
        assert!(close(a, x0) && close(b, y0) && c < x1 && d < y1);
        assert_eq!(lines(&s, id), before);
    }

    #[test]
    fn insets_stay_around_the_text() {
        let mut s = Session::new();
        let id = frame(&mut s, "Hi there");
        fit(&mut s, id, "both");
        let (_, _, w, h) = {
            let (a, b, c, d) = inner(&s, id);
            (a, b, c - a, d - b)
        };
        let mut s2 = Session::new();
        let id2 = frame(&mut s2, "Hi there");
        s2.execute("object.textFrameOptions", &json!({"ids": [id2], "inset": 10})).unwrap();
        fit(&mut s2, id2, "both");
        let (a, b, c, d) = inner(&s2, id2);
        assert!(close(c - a, w + 20.0) && close(d - b, h + 20.0), "{} x {} vs {w} x {h}", c - a, d - b);
    }

    #[test]
    fn several_frames_fit_in_one_undo_step() {
        let mut s = Session::new();
        let a = frame(&mut s, "One");
        let b = text_frame(&mut s, [72.0, 300.0, 400.0, 500.0], "Two");
        let undo = s.doc().unwrap().history.undo.len();
        let (a0, b0) = (inner(&s, a), inner(&s, b));
        let r = s.execute("object.fit", &json!({"mode": "fitFrameToContent", "axes": "both", "ids": [a, b]})).unwrap();
        assert_eq!(r["fitted"], 2);
        assert_eq!(s.doc().unwrap().history.undo.len(), undo + 1);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!((inner(&s, a), inner(&s, b)), (a0, b0));
    }

    #[test]
    fn empty_and_locked_frames_are_left_alone_and_a_bad_axis_is_an_error() {
        let mut s = Session::new();
        let id = frame(&mut s, "");
        let before = inner(&s, id);
        assert_eq!(fit(&mut s, id, "both")["fitted"], 0);
        assert_eq!(inner(&s, id), before);
        let locked = text_frame(&mut s, [72.0, 300.0, 400.0, 500.0], "Locked");
        let d = &mut s.doc_mut().unwrap();
        std::sync::Arc::make_mut(&mut d.doc).item_mut(ItemId(locked)).unwrap().locked = true;
        let before = inner(&s, locked);
        fit(&mut s, locked, "both");
        assert_eq!(inner(&s, locked), before);
        assert!(s.execute("object.fit", &json!({"mode": "fitFrameToContent", "axes": "diagonal", "ids": [id]})).is_err());
    }
}
