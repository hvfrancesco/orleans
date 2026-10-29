//! Shared system fonts for SVG text and DXF text.

use std::sync::{Arc, OnceLock};

use ab_glyph::{Font, FontRef, OutlineCurve};
use kurbo::{Affine, BezPath, Point};
use usvg::fontdb::{Database, Family, Query, Stretch, Style, Weight};

/// Load the system fonts once per process. `usvg` starts with an empty font
/// database, so text-bearing SVGs would otherwise lose every `<text>` element.
pub fn database() -> &'static Arc<Database> {
    static FONTS: OnceLock<Arc<Database>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut database = Database::new();
        database.load_system_fonts();
        Arc::new(database)
    })
}

pub fn usvg_options(dpi: f32) -> usvg::Options<'static> {
    usvg::Options {
        dpi,
        fontdb: database().clone(),
        ..usvg::Options::default()
    }
}

fn font_id(family: &str) -> Option<usvg::fontdb::ID> {
    let database = database();
    let normal = Query {
        families: &[],
        weight: Weight::NORMAL,
        stretch: Stretch::Normal,
        style: Style::Normal,
    };
    let named_family = Family::Name(family);
    let named = Query {
        families: std::slice::from_ref(&named_family),
        ..normal
    };
    database.query(&named).or_else(|| {
        database.query(&Query {
            families: &[Family::SansSerif],
            ..normal
        })
    })
}

/// Run `f` with a parsed system font. The callback may build owned geometry.
pub fn with_font<T>(family: &str, f: impl FnOnce(FontRef<'_>) -> T) -> Option<T> {
    let database = database();
    let id = font_id(family)?;
    database
        .with_face_data(id, |data, index| {
            FontRef::try_from_slice_and_index(data, index).ok().map(f)
        })
        .flatten()
}

/// Outline a string as one path in the caller's coordinate units.
///
/// `height` is the em height in the same units as the returned coordinates.
/// Newlines start a new line; unsupported or missing glyphs still advance the
/// pen so spaces and punctuation keep sensible spacing.
pub fn text_path(
    text: &str,
    family: &str,
    height: f64,
    width_factor: f64,
    oblique_degrees: f64,
) -> Option<BezPath> {
    if text.is_empty() || !height.is_finite() || height <= 0.0 {
        return None;
    }
    let width_factor = if width_factor.is_finite() && width_factor > 0.0 {
        width_factor
    } else {
        1.0
    };
    with_font(family, |font| {
        let units = f64::from(font.units_per_em().unwrap_or(1000.0));
        let scale = height / units;
        let shear = oblique_degrees.to_radians().tan();
        let mut path = BezPath::new();
        let mut pen_x = 0.0;
        let mut pen_y = 0.0;
        let mut any = false;
        for character in text.chars() {
            if character == '\n' {
                pen_x = 0.0;
                pen_y -= height * 1.2;
                continue;
            }
            let glyph = font.glyph_id(character);
            if let Some(outline) = font.outline(glyph) {
                let transform = Affine::translate((pen_x, pen_y))
                    * Affine::new([scale * width_factor, 0.0, shear * scale, scale, 0.0, 0.0]);
                let mut current: Option<Point> = None;
                for curve in outline.curves {
                    let end = match curve {
                        OutlineCurve::Line(p0, p1) => {
                            let start = transform * glyph_point(p0);
                            let end = transform * glyph_point(p1);
                            if current != Some(start) {
                                path.move_to(start);
                            }
                            path.line_to(end);
                            end
                        }
                        OutlineCurve::Quad(p0, p1, p2) => {
                            let start = transform * glyph_point(p0);
                            let control = transform * glyph_point(p1);
                            let end = transform * glyph_point(p2);
                            if current != Some(start) {
                                path.move_to(start);
                            }
                            path.quad_to(control, end);
                            end
                        }
                        OutlineCurve::Cubic(p0, p1, p2, p3) => {
                            let start = transform * glyph_point(p0);
                            let c1 = transform * glyph_point(p1);
                            let c2 = transform * glyph_point(p2);
                            let end = transform * glyph_point(p3);
                            if current != Some(start) {
                                path.move_to(start);
                            }
                            path.curve_to(c1, c2, end);
                            end
                        }
                    };
                    current = Some(end);
                    any = true;
                }
            }
            pen_x += f64::from(font.h_advance_unscaled(glyph)) * scale * width_factor;
        }
        any.then_some(path)
    })
    .flatten()
}

fn glyph_point(point: ab_glyph::Point) -> Point {
    Point::new(f64::from(point.x), f64::from(point.y))
}
