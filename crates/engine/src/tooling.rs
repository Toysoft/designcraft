//! Tool hosting: the active tool receives pointer/key events and its actions are executed here.

use std::sync::Arc;

use designcraft_doc::{CellAddr, StoryId};
use designcraft_geom::Unit;
pub use designcraft_tools::SnapView;
use designcraft_tools::{Action, CanvasLayout, Cursor, Mods, Overlay, PointerEvent, PointerKind, ToolContext, ToolKey};
use serde::Serialize;
use serde_json::Value;

use crate::{EngineError, HistoryEntry, Interaction, Result, Session, record_undo};

/// Something the UI should do in response to a tool or command.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UiRequest {
    Dialog {
        id: String,
        params: Value,
    },
    View {
        params: Value,
    },
    /// Open a file picker for the given purpose (`place`, `open`, `saveAs`, `export`).
    Pick {
        purpose: String,
        params: Value,
    },
}

/// View information the UI passes with pointer events.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ViewInfo {
    /// Screen pixels per point.
    pub zoom: f64,
    pub snap: SnapView,
    pub unit: Unit,
    /// The toolbox's Stroke box is in front (X): colour tools apply to strokes.
    pub stroke_in_front: bool,
}

impl ViewInfo {
    pub const fn at_zoom(zoom: f64) -> Self {
        Self { zoom, snap: SnapView::FACTORY, unit: Unit::Points, stroke_in_front: false }
    }
}

impl Default for ViewInfo {
    fn default() -> Self {
        Self::at_zoom(1.0)
    }
}

impl Session {
    pub fn tool_id(&self) -> &'static str {
        self.tool.id()
    }

    pub fn set_tool(&mut self, id: &str) {
        if self.tool.id() == id {
            return;
        }
        self.end_composition();
        self.tool = designcraft_tools::create(id);
        self.temp_tool = None;
        // Any other tool unloads the place cursor.
        if id != "placeGun" {
            self.loaded = None;
        }
        // Leaving the Type tool keeps the text frame selected.
        if id != "type"
            && let Some(st) = self.active_mut()
            && let Some(t) = st.selection.text.take()
            && let Some(f) = t.frame.or_else(|| st.doc.story(t.story).and_then(|s| s.frames.first().copied()))
        {
            st.selection.items = vec![f];
            st.selection.cells = None;
        }
    }

    pub fn layout(&self) -> CanvasLayout {
        match self.active() {
            Some(st) => CanvasLayout::new(&st.doc, st.editing_parents),
            None => CanvasLayout::default(),
        }
    }

    fn with_ctx<T>(&mut self, view: ViewInfo, f: impl FnOnce(&mut dyn designcraft_tools::Tool, &ToolContext) -> T) -> Option<T> {
        let st = self.active()?;
        let doc = st.doc.clone();
        let sel = st.selection.clone();
        let layer = st.active_layer;
        let layout = CanvasLayout::new(&doc, st.editing_parents);
        let cache = self.cache.clone();
        let cx = ToolContext {
            doc: &doc,
            selection: &sel,
            cache: &cache,
            layout: &layout,
            zoom: view.zoom.max(1e-6),
            layer,
            snap: view.snap,
            unit: view.unit,
            stroke_in_front: view.stroke_in_front,
        };
        // A gesture a Cmd-held press started runs on the borrowed selection tool.
        let tool = self.temp_tool.as_deref_mut().unwrap_or(self.tool.as_mut());
        Some(f(tool, &cx))
    }

    /// Which selection tool a Cmd-held press borrows while `tool` is chosen, as InDesign's
    /// Cmd/Ctrl does: the Direct Selection tool for the Pen family and Pencil, the Selection
    /// tool for the drawing, type and other tools; none for the selection tools themselves,
    /// Hand, Zoom and a loaded place cursor, where Cmd means something else or nothing.
    fn borrowed_selection_tool(tool: &str) -> Option<&'static str> {
        match tool {
            "selection" | "directSelection" | "hand" | "zoom" | "placeGun" => None,
            "pen" | "addAnchor" | "deleteAnchor" | "convertDirection" | "pencil" | "smooth" | "erase" | "scissors" => Some("directSelection"),
            _ => Some("selection"),
        }
    }

    pub fn pointer(&mut self, ev: &PointerEvent, view: ViewInfo) -> Result<()> {
        // Cmd held at a press: the gesture (press, drags, release) goes to the selection tool the
        // chosen tool borrows, which selects, moves or resizes without a tool change (#919).
        if matches!(ev.kind, PointerKind::Down | PointerKind::DoubleClick)
            && ev.mods.cmd
            && self.temp_tool.is_none()
            && !self.tool.busy()
            && let Some(id) = Self::borrowed_selection_tool(self.tool.id())
        {
            self.temp_tool = Some(designcraft_tools::create(id));
        }
        let actions = self.with_ctx(view, |t, cx| t.pointer(cx, ev)).unwrap_or_default();
        let r = self.run_actions(actions);
        if ev.kind == PointerKind::Up && self.temp_tool.as_ref().is_some_and(|t| !t.busy()) {
            self.temp_tool = None;
        }
        r
    }

    pub fn tool_key(&mut self, key: ToolKey, mods: Mods, view: ViewInfo) -> Result<bool> {
        let actions = self.with_ctx(view, |t, cx| t.key(cx, key, mods)).unwrap_or_default();
        let handled = !actions.is_empty();
        self.run_actions(actions)?;
        Ok(handled)
    }

    pub fn overlays(&mut self, view: ViewInfo) -> Vec<Overlay> {
        self.with_ctx(view, |t, cx| t.overlays(cx)).unwrap_or_default()
    }

    pub fn cursor(&mut self, p: designcraft_geom::Point, mods: Mods, view: ViewInfo) -> Cursor {
        // Cmd held over the canvas shows the selection tool a press would borrow.
        if mods.cmd
            && self.temp_tool.is_none()
            && !self.tool.busy()
            && let Some(id) = Self::borrowed_selection_tool(self.tool.id())
        {
            let borrowed = designcraft_tools::create(id);
            return self.with_ctx(view, |_, cx| borrowed.cursor(cx, p, mods)).unwrap_or_default();
        }
        self.with_ctx(view, |t, cx| t.cursor(cx, p, mods)).unwrap_or_default()
    }

    pub fn tool_busy(&self) -> bool {
        self.tool.busy()
    }

    pub fn wants_text(&mut self) -> bool {
        self.with_ctx(ViewInfo::at_zoom(1.0), |t, cx| t.wants_text(cx)).unwrap_or(false)
    }

    pub fn run_actions(&mut self, actions: Vec<Action>) -> Result<()> {
        for a in actions {
            match a {
                Action::Begin(label) => self.begin_interaction(&label)?,
                Action::Preview(cmd, p) => self.preview(&cmd, p)?,
                Action::Commit => self.commit_interaction()?,
                Action::Cancel => self.cancel_interaction(),
                Action::Exec(cmd, p) => {
                    self.execute(&cmd, &p)?;
                }
                Action::Dialog(id, params) => self.ui_requests.push(UiRequest::Dialog { id, params }),
                Action::SwitchTool(id) => self.set_tool(&id),
                Action::View(params) => self.ui_requests.push(UiRequest::View { params }),
            }
        }
        Ok(())
    }

    /// The text an input method is composing at the caret, as story bytes: it is in the
    /// document as a preview until `text.compose` commits or drops it.
    pub fn composition(&self) -> Option<(StoryId, Option<CellAddr>, std::ops::Range<usize>)> {
        let (uid, len) = self.composition?;
        let st = self.active().filter(|st| st.uid == uid)?;
        let t = st.selection.text?;
        Some((t.story, t.cell, t.focus.checked_sub(len)?..t.focus))
    }

    /// Drop the text being composed: the document goes back to how it was before it.
    pub fn end_composition(&mut self) {
        let Some((uid, _)) = self.composition.take() else { return };
        if let Some(st) = self.docs.iter_mut().find(|st| st.uid == uid)
            && let Some(it) = st.interaction.take()
        {
            st.doc = it.doc;
            st.selection = it.selection;
            st.revision += 1;
        }
    }

    pub fn begin_interaction(&mut self, label: &str) -> Result<()> {
        self.end_composition();
        let st = self.doc_mut()?;
        st.interaction = Some(Interaction { label: label.into(), doc: st.doc.clone(), selection: st.selection.clone(), preview: None });
        Ok(())
    }

    /// Apply `cmd` on top of the interaction's snapshot (replacing the previous preview).
    pub fn preview(&mut self, cmd: &str, params: Value) -> Result<()> {
        let st = self.doc_mut()?;
        let Some(it) = st.interaction.clone() else { return Err(EngineError::Other("no interaction".into())) };
        st.doc = it.doc.clone();
        st.selection = it.selection.clone();
        let r = self.execute(cmd, &params);
        if let Some(st) = self.active_mut()
            && let Some(i) = st.interaction.as_mut()
        {
            i.preview = Some((cmd.to_string(), params));
        }
        // Journal only the committed result.
        if r.is_ok() {
            self.journal.pop();
        }
        r.map(|_| ())
    }

    pub fn commit_interaction(&mut self) -> Result<()> {
        let st = self.doc_mut()?;
        let Some(it) = st.interaction.take() else { return Ok(()) };
        if !Arc::ptr_eq(&it.doc, &st.doc) {
            record_undo(st, HistoryEntry { label: it.label, doc: it.doc, selection: it.selection });
        }
        if let Some((c, p)) = it.preview {
            self.journal.push((c, p));
        }
        // The interaction's last preview is the transform to repeat; the next one starts afresh.
        self.transforms.2 = false;
        Ok(())
    }

    pub fn cancel_interaction(&mut self) {
        if let Some(st) = self.active_mut()
            && let Some(it) = st.interaction.take()
        {
            st.doc = it.doc;
            st.selection = it.selection;
            st.revision += 1;
            if self.transforms.2 {
                self.transforms.0.pop();
                self.transforms.2 = false;
            }
        }
    }
}
