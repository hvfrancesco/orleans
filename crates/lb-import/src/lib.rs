//! SVG import.
//!
//! `usvg` resolves the hard parts of SVG (CSS, `use`, transforms, units,
//! shapes-to-paths, text-to-paths with a font database), so this module only
//! has to walk the resolved tree, convert its paths to `kurbo::BezPath` and
//! group them into layers by colour — the same model LightBurn uses.

use std::rc::Rc;

use kurbo::{BezPath, Point};
use lb_core::{CutParams, Document, Layer, Shape};
use usvg::tiny_skia_path::PathSegment;

/// 96 CSS pixels per inch, the SVG/CSS reference resolution.
pub const DEFAULT_DPI: f64 = 96.0;

/// Options controlling how an SVG becomes a [`Document`].
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// Resolution used to interpret SVG user units.
    pub dpi: f64,
    /// SVG's Y axis points down; the machine's points up. Flip it so the
    /// imported design sits in the machine coordinate system.
    pub flip_y: bool,
    /// Uniform scale applied after the unit conversion. `1.0` means "trust the
    /// document's physical size".
    pub scale: f64,
    /// Cutting parameters assigned to every new layer.
    pub default_cut: CutParams,
    /// Group geometry by stroke/fill colour into separate layers.
    pub group_by_color: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            dpi: DEFAULT_DPI,
            flip_y: true,
            scale: 1.0,
            default_cut: CutParams::default(),
            group_by_color: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not parse SVG: {0}")]
    Parse(#[from] usvg::Error),
}

/// Parse SVG bytes into a [`Document`] in millimetres, Y up.
pub fn import_svg(data: &[u8], opt: &ImportOptions) -> Result<Document, ImportError> {
    let usvg_opt = usvg::Options {
        dpi: DEFAULT_DPI as f32,
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(data, &usvg_opt)?;

    let mm_per_px = 25.4 / opt.dpi * opt.scale;
    let width_mm = f64::from(tree.size().width()) * mm_per_px;
    let height_mm = f64::from(tree.size().height()) * mm_per_px;

    let mut doc = Document::new(width_mm, height_mm);
    let mut path_index = 0usize;

    let mut visit = |node: &usvg::Node| {
        let usvg::Node::Path(path) = node else {
            return;
        };
        if !path.is_visible() {
            return;
        }
        let color = shape_color(path);
        let layer = layer_for(&mut doc, color, opt);
        let id = if path.id().is_empty() {
            path_index += 1;
            format!("path-{path_index}")
        } else {
            path.id().to_owned()
        };

        let tf = path.abs_transform();
        let map = |p: usvg::tiny_skia_path::Point| -> Point {
            let mut p = p;
            tf.map_point(&mut p);
            let x = f64::from(p.x) * mm_per_px;
            let y = f64::from(p.y) * mm_per_px;
            let y = if opt.flip_y { height_mm - y } else { y };
            Point::new(x, y)
        };

        let mut bez = BezPath::new();
        let mut any = false;
        for seg in path.data().segments() {
            match seg {
                PathSegment::MoveTo(p) => {
                    bez.move_to(map(p));
                    any = true;
                }
                PathSegment::LineTo(p) => bez.line_to(map(p)),
                PathSegment::QuadTo(c, p) => bez.quad_to(map(c), map(p)),
                PathSegment::CubicTo(c1, c2, p) => bez.curve_to(map(c1), map(c2), map(p)),
                PathSegment::Close => bez.close_path(),
            }
        }
        if !any {
            return;
        }

        doc.layers[layer]
            .shapes
            .push(Shape::new(id, bez, path.fill().is_some()));
    };
    walk(tree.root().children(), &mut visit);

    Ok(doc)
}

/// Depth-first walk over the resolved node tree.
fn walk(nodes: &[usvg::Node], f: &mut impl FnMut(&usvg::Node)) {
    for node in nodes {
        f(node);
        if let usvg::Node::Group(group) = node {
            walk(group.children(), f);
        }
    }
}

fn shape_color(path: &usvg::Path) -> [u8; 3] {
    let paint = path
        .stroke()
        .map(|s| s.paint())
        .or_else(|| path.fill().map(|f| f.paint()));
    match paint {
        Some(usvg::Paint::Color(c)) => [c.red, c.green, c.blue],
        // Gradients and patterns have no single colour; engrave them as black.
        _ => [0, 0, 0],
    }
}

fn layer_for(doc: &mut Document, color: [u8; 3], opt: &ImportOptions) -> usize {
    if opt.group_by_color {
        if let Some(idx) = doc.layers.iter().position(|l| l.color == color) {
            return idx;
        }
    } else if let Some(last) = doc.layers.last() {
        return last.index;
    }

    let index = doc.layers.len();
    let name = if opt.group_by_color {
        format!("C{:02}", index)
    } else {
        "Cut".to_owned()
    };
    doc.layers
        .push(Layer::new(index, name, color, opt.default_cut));
    index
}

/// Convenience wrapper used by the CLI; keeps `Rc` out of the public API.
#[allow(dead_code)]
fn _assert_send(_: Rc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_for(svg: &str) -> Document {
        import_svg(svg.as_bytes(), &ImportOptions::default()).expect("import")
    }

    #[test]
    fn imports_rect_in_millimetres() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm">
            <rect x="1mm" y="2mm" width="4mm" height="3mm" fill="none" stroke="#ff0000"/>
        </svg>"##;
        let doc = doc_for(svg);

        assert!((doc.width - 10.0).abs() < 1e-3, "width {}", doc.width);
        assert!((doc.height - 10.0).abs() < 1e-3);
        assert_eq!(doc.shape_count(), 1);
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].color, [255, 0, 0]);

        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x0 - 1.0).abs() < 1e-3, "{bounds:?}");
        assert!((bounds.x1 - 5.0).abs() < 1e-3);
        // SVG y=2mm from the top becomes y=8mm from the bottom.
        assert!((bounds.y0 - 5.0).abs() < 1e-3, "{bounds:?}");
        assert!((bounds.y1 - 8.0).abs() < 1e-3);
    }

    #[test]
    fn groups_by_colour() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm">
            <rect x="0" y="0" width="5" height="5" fill="none" stroke="#00ff00"/>
            <rect x="8" y="0" width="5" height="5" fill="#00ff00" stroke="none"/>
            <circle cx="10" cy="10" r="3" fill="none" stroke="#0000ff"/>
        </svg>"##;
        let doc = doc_for(svg);
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.shape_count(), 3);
    }

    #[test]
    fn curve_shapes_survive_as_beziers() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm">
            <circle cx="5mm" cy="5mm" r="4mm" fill="none" stroke="black"/>
        </svg>"##;
        let doc = doc_for(svg);
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.width() - 8.0).abs() < 0.05, "{}", bounds.width());
    }
}
