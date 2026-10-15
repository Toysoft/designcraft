//! Trace a transparent image into a fixed text-wrap boundary without changing its frame.
use designcraft_doc::{ContourType, ItemId, WrapMode};
use designcraft_geom::{Affine, BezPath, PathData, Point, pathfinder};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_doc};
use crate::{Result, Session};

const ID: &str = "object.traceTransparencyWrap";
const MAX_PIXELS: u64 = 16_000_000;
const MAX_GRID: u32 = 128;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "object.traceTransparencyWrap",
        "Trace Transparency for Text Wrap…",
        [],
        None,
        "{id?: selected image frame, offset?: 6 (0..72 pt), threshold?: 16 (0..254)} → {id, anchors} — creates a fixed contour around the visible raster; trace again after changing the image content; no subject detection",
        has_doc,
        trace
    )]
}

fn trace(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.doc()?;
    let id = match p.get("id") {
        Some(v) => ItemId(v.as_u64().ok_or_else(|| bad(ID, "id must be an image frame id"))?),
        None if st.selection.items.len() == 1 => st.selection.items.first().copied().ok_or_else(|| bad(ID, "select one image frame"))?,
        None => return Err(bad(ID, "select one image frame")),
    };
    let offset = match p.get("offset") {
        None => 6.0,
        Some(v) => v.as_f64().filter(|n| n.is_finite() && (0.0..=72.0).contains(n)).ok_or_else(|| bad(ID, "offset must be 0 to 72 points"))?,
    };
    let threshold = match p.get("threshold") {
        None => 16,
        Some(v) => v.as_u64().filter(|n| *n <= 254).ok_or_else(|| bad(ID, "threshold must be an integer from 0 to 254"))? as u8,
    };
    super::object::path_editable(&st.doc, id, ID)?;
    let it = st.doc.item(id).ok_or_else(|| bad(ID, "no such image frame"))?;
    let graphic = it.graphic().ok_or_else(|| bad(ID, "select a placed raster image"))?;
    let asset = st.doc.assets.get(&graphic.asset).filter(|a| a.is_raster()).ok_or_else(|| bad(ID, "select a placed raster image"))?;
    let (w, h) = designcraft_images::pixel_size(&asset.data).ok_or_else(|| bad(ID, "cannot read image dimensions"))?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(bad(ID, "the source must contain between 1 and 16 million pixels"));
    }
    if it.path.anchor_count() > 256 {
        return Err(bad(ID, "the clipping frame is too complex to trace"));
    }
    let img = designcraft_images::decode_rgba(&asset.data).ok_or_else(|| bad(ID, "cannot decode the image"))?;
    let (cols, rows) = (w.min(MAX_GRID), h.min(MAX_GRID));
    let mut mask = vec![false; (cols * rows) as usize];
    let mut transparent = false;
    // Conservative occupancy: every visible source pixel marks a cell in a bounded grid.
    for (y, row) in img.rows().enumerate() {
        let gy = y as u64 * u64::from(rows) / u64::from(h);
        for (x, pixel) in row.enumerate() {
            let alpha = pixel.0.get(3).copied().unwrap_or(0);
            transparent |= alpha < 255;
            if alpha > threshold {
                let gx = x as u64 * u64::from(cols) / u64::from(w);
                if let Some(cell) = mask.get_mut((gy * u64::from(cols) + gx) as usize) {
                    *cell = true;
                }
            }
        }
    }
    if !transparent {
        return Err(bad(ID, "the image is opaque; subject detection is not available"));
    }
    let xf = graphic.xf * Affine::scale_non_uniform(graphic.size.0 / f64::from(cols), graphic.size.1 / f64::from(rows));
    if !xf.as_coeffs().iter().all(|n| n.is_finite()) || xf.determinant().abs() < 1e-12 {
        return Err(bad(ID, "the image transform is degenerate"));
    }
    let outline = mask_path(&mask, cols as usize, rows as usize, xf)?;
    if outline.is_empty() {
        return Err(bad(ID, "no visible pixels exceed the threshold"));
    }
    let frame = PathData::from_bezpath(&designcraft_geom::corners::apply(&it.path, &it.corners));
    let valid = |p: &PathData| {
        p.subpaths
            .iter()
            .flat_map(|s| &s.anchors)
            .flat_map(|a| [a.p, a.h_in, a.h_out])
            .all(|p| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1_000_000.0 && p.y.abs() <= 1_000_000.0)
    };
    if !valid(&outline) || !valid(&frame) {
        return Err(bad(ID, "the traced geometry is outside the supported range"));
    }
    let clipped = pathfinder::combine(pathfinder::Op::Intersect, &[outline, frame]).ok_or_else(|| bad(ID, "the frame shows no traced pixels"))?;
    let mut path = clipped.to_bezpath();
    if offset > 0.0 {
        let stroke = designcraft_geom::kurbo::Stroke::new(offset * 2.0).with_join(designcraft_geom::kurbo::Join::Bevel);
        let border = designcraft_geom::kurbo::stroke(path.iter(), &stroke, &designcraft_geom::kurbo::StrokeOpts::default(), 0.25);
        path.extend(border.iter());
    }
    let path = PathData::from_bezpath(&path);
    if path.anchor_count() > 6000 || path.subpaths.iter().flat_map(|s| &s.anchors).any(|a| !a.p.x.is_finite() || !a.p.y.is_finite()) {
        return Err(bad(ID, "the traced boundary is too complex"));
    }
    let anchors = path.anchor_count();
    s.edit(|doc, _| {
        let it = doc.item_mut(id).ok_or_else(|| bad(ID, "the image frame is unavailable"))?;
        it.wrap.mode = WrapMode::Contour;
        it.wrap.contour = ContourType::GraphicFrame;
        it.wrap.offsets = [offset; 4];
        it.wrap.invert = false;
        it.wrap.path = Some(path);
        Ok(json!({"id":id.0,"anchors":anchors}))
    })
}

fn mask_path(mask: &[bool], cols: usize, rows: usize, xf: Affine) -> Result<PathData> {
    use std::collections::BTreeMap;
    type Vertex = (i32, i32);
    let occupied = |x: i32, y: i32| {
        x >= 0 && y >= 0 && (x as usize) < cols && (y as usize) < rows && mask.get(y as usize * cols + x as usize).copied().unwrap_or(false)
    };
    let mut edges: BTreeMap<Vertex, Vec<Vertex>> = BTreeMap::new();
    let mut count = 0usize;
    for y in 0..rows {
        for x in 0..cols {
            let (x, y) = (x as i32, y as i32);
            if !occupied(x, y) {
                continue;
            }
            for (outside, a, b) in [
                (!occupied(x, y - 1), (x, y), (x + 1, y)),
                (!occupied(x + 1, y), (x + 1, y), (x + 1, y + 1)),
                (!occupied(x, y + 1), (x + 1, y + 1), (x, y + 1)),
                (!occupied(x - 1, y), (x, y + 1), (x, y)),
            ] {
                if outside {
                    count += 1;
                    if count > 4096 {
                        return Err(bad(ID, "the image transparency is too complex"));
                    }
                    edges.entry(a).or_default().push(b);
                }
            }
        }
    }
    let mut path = BezPath::new();
    while let Some((&start, _)) = edges.first_key_value() {
        let mut current = start;
        let mut points = vec![start];
        let mut closed = false;
        for _ in 0..=4096 {
            let next = edges.get_mut(&current).and_then(|v| v.pop()).ok_or_else(|| bad(ID, "the transparency boundary is incomplete"))?;
            if edges.get(&current).is_some_and(Vec::is_empty) {
                edges.remove(&current);
            }
            if let Some((&a, &b)) = points.len().checked_sub(2).and_then(|i| points.get(i).zip(points.last()))
                && (b.0 - a.0) * (next.1 - b.1) == (b.1 - a.1) * (next.0 - b.0)
            {
                points.pop();
            }
            points.push(next);
            current = next;
            if current == start {
                closed = true;
                break;
            }
        }
        if !closed {
            return Err(bad(ID, "the transparency boundary is too long"));
        }
        if let Some(&first) = points.first() {
            path.move_to(xf * Point::new(f64::from(first.0), f64::from(first.1)));
            for &(x, y) in points.iter().skip(1) {
                path.line_to(xf * Point::new(f64::from(x), f64::from(y)));
            }
            path.close_path();
        }
    }
    Ok(PathData::from_bezpath(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caption_fits_between_traced_legs_without_ignoring_wrap() {
        let mut pixels = vec![0; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                if (8..20).contains(&x) || (44..56).contains(&x) {
                    pixels[(y * 64 + x) * 4 + 3] = 255;
                }
            }
        }
        let png = designcraft_render::Rendered { width: 64, height: 64, pixels }.to_png();
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let image = s
            .execute("file.place", &json!({"base64":super::super::file::base64_encode(&png),"name":"legs.png","x":200,"y":100,"width":100}))
            .unwrap()["id"]
            .clone();
        let caption = s.execute("frame.create", &json!({"rect":[240,120,260,160],"content":"text","text":"Hi"})).unwrap();
        s.execute("selection.set", &json!({"ids":[caption["id"]]})).unwrap();
        s.execute("type.char", &json!({"size":8})).unwrap();
        s.execute("object.textWrap", &json!({"ids":[image],"mode":"contour"})).unwrap();
        let overset =
            |s: &Session| s.cache.get(&s.doc().unwrap().doc, designcraft_doc::StoryId(caption["story"].as_u64().unwrap()), None).is_overset();
        assert!(overset(&s));
        s.execute(ID, &json!({"id":image,"offset":0})).unwrap();
        assert!(!overset(&s), "transparent space between the legs remains available to the caption");
    }

    fn image(alpha: bool) -> (Session, Value) {
        let mut pixels = vec![0; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let i = (y * 64 + x) * 4;
                pixels[i] = 150;
                pixels[i + 1] = 80;
                pixels[i + 3] = if !alpha || (16..48).contains(&x) { 255 } else { 0 };
            }
        }
        let png = designcraft_render::Rendered { width: 64, height: 64, pixels }.to_png();
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        let placed = s
            .execute("file.place", &json!({"base64":super::super::file::base64_encode(&png),"name":"cutout.png","x":200,"y":100,"width":100}))
            .unwrap();
        (s, placed["id"].clone())
    }

    #[test]
    fn trace_keeps_image_and_frame_unchanged_and_round_trips_with_undo() {
        let (mut s, id) = image(true);
        let before = s.doc().unwrap().doc.clone();
        s.execute(ID, &json!({"id":id,"offset":6})).unwrap();
        let d = s.doc().unwrap().doc.clone();
        let it = d.item(ItemId(id.as_u64().unwrap())).unwrap();
        let old = before.item(it.id).unwrap();
        assert_eq!(it.path, old.path);
        assert_eq!(it.content, old.content);
        let bounds = it.wrap.path.as_ref().unwrap().bounds().unwrap();
        assert!((bounds.x0 - 219.0).abs() < 0.1 && (bounds.x1 - 281.0).abs() < 0.1, "{bounds:?}");
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.doc().unwrap().doc, before);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.doc().unwrap().doc, d);
        let back = designcraft_idml::import_idml(&designcraft_idml::export_idml(&d)).unwrap();
        assert!(back.spreads[0].items.iter().any(|i| i.wrap.path.is_some()));
    }

    #[test]
    fn invalid_opaque_and_locked_images_leave_the_document_unchanged() {
        let (mut s, id) = image(false);
        let before = s.doc().unwrap().doc.clone();
        assert!(s.execute(ID, &json!({"id":id})).is_err());
        assert_eq!(s.doc().unwrap().doc, before);
        let (mut s, id) = image(true);
        let before = s.doc().unwrap().doc.clone();
        for p in [json!({"id":id,"offset":-1}), json!({"id":id,"threshold":255}), json!({"id":id,"threshold":0.5}), json!({"id":99999})] {
            assert!(s.execute(ID, &p).is_err());
            assert_eq!(s.doc().unwrap().doc, before);
        }
        s.execute("object.lock", &json!({"ids":[id]})).unwrap();
        let before = s.doc().unwrap().doc.clone();
        assert!(s.execute(ID, &json!({"id":id})).is_err());
        assert_eq!(s.doc().unwrap().doc, before);
    }
}
