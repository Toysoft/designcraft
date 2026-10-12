//! Type tool (T): click in a frame to place the caret, drag in a frame to select text, drag on
//! empty canvas to draw a new text frame. Keys move the caret / delete; typing goes through
//! `text.insert` (sent by the UI as text input). Dragging a table's row or column border resizes
//! it (`table.resizeBorder`).

use designcraft_compose::BorderHit;
use designcraft_geom::{Affine, Point};
use serde_json::json;

use crate::{Action, Cursor, Mods, PointerEvent, PointerKind, Tool, ToolContext, ToolKey, frame::drag_rect, rect_json, spread_json};

#[derive(Default)]
pub struct TypeTool {
    /// Vertical Type Tool: frames it draws set text vertically.
    pub vertical: bool,
    start: Option<Point>,
    selecting: Option<u64>,
    drawing: bool,
    /// A press on a table whose whole rows or columns are selected: (frame, press point in
    /// spread space) — a drag moves them, a click places the caret.
    cell_drag: Option<(u64, Point)>,
    /// A press on a table row or column border: a drag resizes it.
    border: Option<BorderDrag>,
}

/// A table border under the pointer, and the frame's canvas → inner-space mapping.
#[derive(Clone, Copy)]
struct BorderDrag {
    frame: u64,
    story: u64,
    hit: BorderHit,
    /// Canvas → frame inner space (through rotations, scaling and turned spreads).
    inv: Affine,
    /// Press point in frame inner space.
    start: Point,
    begun: bool,
    /// The border runs vertically on the canvas (column borders on an unrotated frame).
    horizontal_drag: bool,
}

/// The row or column border of a table near canvas point `pos`, in an unlocked text frame.
fn border_at(cx: &ToolContext, pos: Point) -> Option<BorderDrag> {
    let (_, id) = cx.hit(pos)?;
    let it = cx.doc.item(id)?;
    if it.locked {
        return None;
    }
    let sid = it.text_frame()?.story;
    let story = cx.doc.story(sid)?;
    if story.tables.is_empty() {
        return None;
    }
    let xf = cx.item_canvas_xf(id)? * cx.doc.text_xf(it);
    let det = xf.determinant().abs();
    if !(det.is_finite() && det > 1e-12) {
        return None;
    }
    let inv = xf.inverse();
    let start = inv * pos;
    let cs = cx.cache.get(cx.doc, sid, None);
    let fi = cs.frames.iter().position(|f| f.frame == id)?;
    let hit = designcraft_compose::hit_border(&cs, fi, &story.tables, start, cx.tol(4.0) / det.sqrt())?;
    let axis = if hit.column { Point::new(1.0, 0.0) } else { Point::new(0.0, 1.0) };
    let v = (xf * axis) - (xf * Point::ORIGIN);
    Some(BorderDrag { frame: id.0, story: sid.0, hit, inv, start, begun: false, horizontal_drag: v.x.abs() >= v.y.abs() })
}

impl BorderDrag {
    fn cursor(&self) -> Cursor {
        if self.horizontal_drag { Cursor::ResizeH } else { Cursor::ResizeV }
    }

    /// Begin (once) and preview the resize for the pointer at canvas `pos`.
    fn drag(&mut self, pos: Point, keep: bool) -> Vec<Action> {
        let d = (self.inv * pos) - self.start;
        let mut delta = if self.hit.column { d.x } else { d.y };
        if self.hit.mirrored {
            delta = -delta;
        }
        if !delta.is_finite() {
            return vec![];
        }
        let mut out = vec![];
        if !self.begun {
            self.begun = true;
            out.push(Action::Begin(if self.hit.column { "Resize Column" } else { "Resize Row" }.into()));
        }
        out.push(Action::Preview(
            "table.resizeBorder".into(),
            json!({
                "frame": self.frame,
                "story": self.story,
                "table": self.hit.table,
                "axis": if self.hit.column { "column" } else { "row" },
                "index": self.hit.index,
                "delta": delta,
                "keepTableSize": keep,
            }),
        ));
        out
    }
}

/// Are whole rows or whole columns of a table selected?
fn whole_rows_or_cols(cx: &ToolContext) -> bool {
    let Some(ts) = cx.selection.cells else { return false };
    let Some(t) = cx.doc.story(ts.story).and_then(|s| s.tables.get(&ts.table)) else { return false };
    let r = ts.range;
    (r.c0 == 0 && r.c1 + 1 == t.ncols()) || (r.r0 == 0 && r.r1 + 1 == t.nrows())
}

impl TypeTool {
    pub fn vertical() -> Self {
        TypeTool { vertical: true, ..Default::default() }
    }
}

impl Tool for TypeTool {
    fn id(&self) -> &'static str {
        if self.vertical { "verticalType" } else { "type" }
    }

    fn pointer(&mut self, cx: &ToolContext, ev: &PointerEvent) -> Vec<Action> {
        match ev.kind {
            PointerKind::Down | PointerKind::DoubleClick => {
                self.start = Some(ev.pos);
                self.drawing = false;
                self.selecting = None;
                self.cell_drag = None;
                self.border = None;
                if ev.kind == PointerKind::Down
                    && let Some(b) = border_at(cx, ev.pos)
                {
                    self.border = Some(b);
                    return vec![];
                }
                if let Some((_, id)) = cx.hit(ev.pos)
                    && let Some(it) = cx.doc.item(id)
                    && !matches!(it.content, designcraft_doc::Content::Graphic(_) | designcraft_doc::Content::Group { .. })
                {
                    let sp = cx.layout.spread_at(ev.pos).map(|(_, p)| p).unwrap_or(ev.pos);
                    // Whole rows/columns selected: wait to see whether this is a drag.
                    if ev.kind == PointerKind::Down && !ev.mods.shift && whole_rows_or_cols(cx) {
                        self.cell_drag = Some((id.0, sp));
                        return vec![];
                    }
                    self.selecting = Some(id.0);
                    let cmd = if ev.kind == PointerKind::DoubleClick {
                        "text.selectWord"
                    } else if ev.mods.shift {
                        "text.extendTo"
                    } else {
                        "text.placeCaret"
                    };
                    return vec![Action::Exec(cmd.into(), json!({"frame": id.0, "point": [sp.x, sp.y]}))];
                }
                vec![]
            }
            PointerKind::Drag => {
                let Some(a) = self.start else { return vec![] };
                if let Some(b) = self.border.as_mut() {
                    // A click with a little wobble must not resize.
                    if !b.begun && (ev.pos - a).hypot() < cx.tol(3.0) {
                        return vec![];
                    }
                    return b.drag(ev.pos, ev.mods.shift);
                }
                if self.cell_drag.is_some() {
                    return vec![];
                }
                if let Some(fid) = self.selecting {
                    let sp = cx.layout.spread_at(ev.pos).map(|(_, p)| p).unwrap_or(ev.pos);
                    return vec![Action::Exec("text.extendTo".into(), json!({"frame": fid, "point": [sp.x, sp.y]}))];
                }
                if (ev.pos - a).hypot() < cx.tol(3.0) && !self.drawing {
                    return vec![];
                }
                let Some((sr, sa)) = cx.layout.spread_at(a) else { return vec![] };
                let r = drag_rect(sa, cx.layout.to_spread(sr, ev.pos), ev.mods);
                let mut out = vec![];
                if !self.drawing {
                    self.drawing = true;
                    out.push(Action::Begin("Create Text Frame".into()));
                }
                out.push(Action::Preview(
                    "frame.create".into(),
                    json!({"spread": spread_json(sr), "shape": "rectangle", "content": "text", "rect": rect_json(r), "caret": true, "vertical": self.vertical}),
                ));
                out
            }
            PointerKind::Up => {
                let start = self.start.take();
                if let Some(b) = self.border.take() {
                    return if b.begun { vec![Action::Commit] } else { vec![] };
                }
                if let Some((fid, from)) = self.cell_drag.take() {
                    let sp = cx.layout.spread_at(ev.pos).map(|(_, p)| p).unwrap_or(ev.pos);
                    let moved = start.is_some_and(|a| (ev.pos - a).hypot() >= cx.tol(3.0));
                    return if moved {
                        vec![Action::Exec("table.dropCells".into(), json!({"frame": fid, "from": [from.x, from.y], "to": [sp.x, sp.y]}))]
                    } else {
                        vec![Action::Exec("text.placeCaret".into(), json!({"frame": fid, "point": [from.x, from.y]}))]
                    };
                }
                if std::mem::take(&mut self.drawing) {
                    self.selecting = None;
                    return vec![Action::Commit];
                }
                // Ends a press in text: drops dragged text, or is ignored by the engine.
                if let Some(fid) = self.selecting.take() {
                    let sp = cx.layout.spread_at(ev.pos).map(|(_, p)| p).unwrap_or(ev.pos);
                    let moved = start.is_some_and(|a| (ev.pos - a).hypot() >= cx.tol(3.0));
                    return vec![Action::Exec(
                        "text.release".into(),
                        json!({"frame": fid, "point": [sp.x, sp.y], "moved": moved, "copy": ev.mods.alt}),
                    )];
                }
                vec![]
            }
            PointerKind::Move => vec![],
        }
    }

    fn key(&mut self, cx: &ToolContext, key: ToolKey, mods: Mods) -> Vec<Action> {
        if key == ToolKey::Escape
            && let Some(b) = self.border.take()
        {
            self.start = None;
            return if b.begun { vec![Action::Cancel] } else { vec![] };
        }
        if cx.selection.text.is_none() {
            return vec![];
        }
        let mv = |dir: &str| {
            vec![Action::Exec("text.move".into(), json!({"dir": dir, "extend": mods.shift, "word": mods.alt || mods.ctrl, "far": mods.cmd}))]
        };
        match key {
            ToolKey::Left => mv("left"),
            ToolKey::Right => mv("right"),
            ToolKey::Up => mv("up"),
            ToolKey::Down => mv("down"),
            ToolKey::Home => mv("lineStart"),
            ToolKey::End => mv("lineEnd"),
            ToolKey::Backspace => vec![Action::Exec("text.delete".into(), json!({"forward": false, "word": mods.alt}))],
            ToolKey::Delete => vec![Action::Exec("text.delete".into(), json!({"forward": true, "word": mods.alt}))],
            ToolKey::Enter => vec![Action::Exec("text.insert".into(), json!({"text": if mods.shift { "\u{2028}" } else { "\n" }}))],
            ToolKey::Tab => vec![Action::Exec("text.insert".into(), json!({"text": "\t"}))],
            ToolKey::Escape => vec![Action::SwitchTool("selection".into()), Action::Exec("text.exitToFrame".into(), json!({}))],
            _ => vec![],
        }
    }

    fn cursor(&self, cx: &ToolContext, p: Point, _m: Mods) -> Cursor {
        if let Some(b) = self.border.as_ref().copied().or_else(|| if self.start.is_none() { border_at(cx, p) } else { None }) {
            return b.cursor();
        }
        Cursor::Text
    }

    fn busy(&self) -> bool {
        self.drawing || self.border.is_some()
    }

    fn wants_text(&self, cx: &ToolContext) -> bool {
        cx.selection.text.is_some()
    }
}
