//! A text stroke aligned outside (InDesign's stroke behind the fill) leaves the glyph's fill
//! whole and puts its full weight outside the outline, on screen and in exported PDF.
use designcraft_color::{Color, Swatch};
use designcraft_compose::Cache;
use designcraft_doc::{CharAttrs, Document, ParaFormat, SpreadRef, StrokeAlign, build::NewDocument};
use designcraft_geom::{Affine, Rect};
use designcraft_pdf::{PdfOptions, export_pdf};
use designcraft_render::{Placed, RenderOptions, Rendered, Renderer};

fn document(stroke: Option<StrokeAlign>) -> Document {
    let mut d = Document::new(&NewDocument { width: 240.0, height: 240.0, facing_pages: false, ..Default::default() });
    d.swatches.push(Swatch::color("Cyan rule", Color::rgb(0.0, 1.0, 1.0)));
    let chars = CharAttrs {
        font_family: Some("Source Sans 3".into()),
        size: Some(120.0),
        fill: Some("[Black]".into()),
        stroke: Some(if stroke.is_some() { "Cyan rule" } else { "[None]" }.into()),
        stroke_weight: Some(6.0),
        stroke_alignment: stroke,
        ..Default::default()
    };
    d.add_text_frame(SpreadRef::Doc(0), Rect::new(20.0, 20.0, 220.0, 220.0), d.default_layer(), "HI", ParaFormat { chars, ..Default::default() })
        .unwrap();
    d
}

fn raster(d: &Document) -> Rendered {
    let placed = [Placed { spread: SpreadRef::Doc(0), xf: Affine::IDENTITY }];
    let options = RenderOptions { paper: false, background: Some([255; 4]), greek_below_px: 0.0, ..Default::default() };
    Renderer::new().render(d, &Cache::new(), &placed, 480, 480, Affine::scale(2.0), &options)
}

fn pdf(d: &Document) -> Rendered {
    let bytes = export_pdf(d, &Cache::new(), &PdfOptions::default()).unwrap();
    let image = designcraft_render::render_pdf_page(&bytes, 0, 480).unwrap();
    let pixels = image
        .data()
        .iter()
        .flat_map(|p| {
            let white = 255 - p.a;
            [p.r.saturating_add(white), p.g.saturating_add(white), p.b.saturating_add(white), 255]
        })
        .collect();
    Rendered { width: 480, height: 480, pixels }
}

fn dark(p: [u8; 4]) -> bool {
    p[0] < 80 && p[1] < 80 && p[2] < 80
}
fn white(p: [u8; 4]) -> bool {
    p[0] > 245 && p[1] > 245 && p[2] > 245
}
fn cyan(p: [u8; 4]) -> bool {
    p[0] < 35 && p[1] > 220 && p[2] > 220
}

fn check(render: impl Fn(&Document) -> Rendered) {
    let plain = render(&document(None));
    let center = render(&document(Some(StrokeAlign::Center)));
    let outside = render(&document(Some(StrokeAlign::Outside)));
    let (mut covered_fill, mut wider) = (0, 0);
    for y in 0..480 {
        for x in 0..480 {
            let (p, c, o) = (plain.pixel(x, y), center.pixel(x, y), outside.pixel(x, y));
            if dark(p) && cyan(c) {
                covered_fill += 1;
                assert!(dark(o), "an outside stroke must stay behind the fill at {x},{y}: {o:?}");
            }
            if white(p) && cyan(c) {
                assert!(cyan(o), "an outside stroke must cover the centred stroke's outer half at {x},{y}: {o:?}");
            }
            if white(p) && white(c) && cyan(o) {
                wider += 1;
            }
        }
    }
    assert!(covered_fill > 50, "fixture needs fill pixels the centred stroke covers, got {covered_fill}");
    assert!(wider > 50, "an outside stroke must reach its full weight past the outline, got {wider}");
}

#[test]
fn raster_outside_text_stroke_is_behind_the_fill() {
    check(raster);
}

#[test]
fn pdf_outside_text_stroke_is_behind_the_fill() {
    check(pdf);
}
