//! PDF / PDF-compatible Illustrator import.
//!
//! This is intentionally a vector-path importer, not a PDF renderer. It walks
//! the first page's content streams, follows form XObjects, and turns painted
//! paths into shapes. Text, raster images, clipping, gradients and patterns are
//! outside the scope: text must be converted to outlines in the source file.

use std::fmt::Display;

use kurbo::{BezPath, PathEl, Point, Shape as _};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as PdfDocument, Object, ObjectId, Stream};
use orleans_core::{Document, Shape};

use crate::{layer_for, ImportError, ImportOptions};

const MM_PER_POINT: f64 = 25.4 / 72.0;
const MAX_FORM_DEPTH: usize = 12;

/// Parse a PDF (or a PDF-compatible `.ai` file) into millimetres, Y up.
///
/// Import one page (`opt.pdf_page`, zero-based). PDF vector paths are grouped
/// by their stroke colour, or by fill colour when they have no stroke. PDF
/// coordinates are already physical and Y-up, so `dpi` and `flip_y` do not
/// apply. The default page is the first one.
pub fn import_pdf(data: &[u8], opt: &ImportOptions) -> Result<Document, ImportError> {
    if !looks_like_pdf(data) {
        return Err(ImportError::Pdf(
            "not a PDF stream; save Illustrator files with PDF compatibility enabled".to_owned(),
        ));
    }

    let pdf = PdfDocument::load_mem(data).map_err(pdf_error)?;
    let pages = pdf.get_pages();
    let page_count = pages.len();
    let (_, page_id) = pages.into_iter().nth(opt.pdf_page).ok_or_else(|| {
        ImportError::Pdf(format!(
            "PDF page {} does not exist (document has {page_count} page(s))",
            opt.pdf_page + 1
        ))
    })?;
    let media = page_box(&pdf, page_id, b"MediaBox")
        .ok_or_else(|| ImportError::Pdf("PDF page has no valid MediaBox".to_owned()))?;
    let crop = page_box(&pdf, page_id, b"CropBox")
        .filter(|[x0, y0, x1, y1]| x1 > x0 && y1 > y0)
        .unwrap_or(media);
    let rotation = page_rotation(&pdf, page_id);
    let user_unit = page_number(&pdf, page_id, b"UserUnit")
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    let scale = if opt.scale.is_finite() && opt.scale > 0.0 {
        opt.scale
    } else {
        1.0
    } * MM_PER_POINT
        * user_unit;

    let (page_matrix, width, height) = page_matrix(crop, rotation, scale);
    let content = pdf
        .get_and_decode_page_content(page_id)
        .map_err(pdf_error)?;
    let resources = inherited_resources(&pdf, page_id);

    let mut importer = PdfImporter {
        pdf: &pdf,
        doc: Document::new(width, height),
        opt,
        next_shape: 0,
    };
    let state = GraphicsState {
        ctm: page_matrix,
        fill: [0, 0, 0],
        stroke: [0, 0, 0],
    };
    importer.parse_operations(&content.operations, state, resources, 0);

    if importer.doc.is_empty() {
        return Err(ImportError::Pdf(
            "no vector paths found; outline text and remove raster-only or clipped artwork"
                .to_owned(),
        ));
    }
    Ok(importer.doc)
}

fn looks_like_pdf(data: &[u8]) -> bool {
    data.len() >= 5
        && data[..data.len().min(1024)]
            .windows(5)
            .any(|window| window == b"%PDF-")
}

/// Number of pages in a PDF / PDF-compatible AI file.
pub fn pdf_page_count(data: &[u8]) -> Result<usize, ImportError> {
    if !looks_like_pdf(data) {
        return Err(ImportError::Pdf(
            "not a PDF stream; save Illustrator files with PDF compatibility enabled".to_owned(),
        ));
    }
    let pdf = PdfDocument::load_mem(data).map_err(pdf_error)?;
    let count = pdf.get_pages().len();
    if count == 0 {
        return Err(ImportError::Pdf("PDF contains no pages".to_owned()));
    }
    Ok(count)
}

fn pdf_error(error: impl Display) -> ImportError {
    ImportError::Pdf(error.to_string())
}

fn page_box(pdf: &PdfDocument, page_id: ObjectId, key: &[u8]) -> Option<[f64; 4]> {
    let (_, value) = pdf.dereference(page_value(pdf, page_id, key)?).ok()?;
    let values = value.as_array().ok()?;
    if values.len() != 4 {
        return None;
    }
    let mut box_ = [0.0; 4];
    for (slot, value) in box_.iter_mut().zip(values) {
        let number = f64::from(value.as_float().ok()?);
        if !number.is_finite() {
            return None;
        }
        *slot = number;
    }
    let [x0, y0, x1, y1] = box_;
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

fn page_rotation(pdf: &PdfDocument, page_id: ObjectId) -> i64 {
    page_number(pdf, page_id, b"Rotate")
        .map(|rotation| rotation.round() as i64)
        .unwrap_or(0)
        .rem_euclid(360)
}

fn page_number(pdf: &PdfDocument, page_id: ObjectId, key: &[u8]) -> Option<f64> {
    let (_, value) = pdf.dereference(page_value(pdf, page_id, key)?).ok()?;
    value.as_float().ok().map(f64::from)
}

fn page_value<'a>(pdf: &'a PdfDocument, page_id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = Some(page_id);
    for _ in 0..32 {
        let page = pdf.get_dictionary(id?).ok()?;
        if let Ok(value) = page.get(key) {
            return Some(value);
        }
        id = page
            .get(b"Parent")
            .ok()
            .and_then(|parent| pdf.dereference(parent).ok())
            .and_then(|(parent_id, _)| parent_id);
    }
    None
}

/// Return the nearest `/Resources` dictionary, following the page tree.
fn inherited_resources(pdf: &PdfDocument, page_id: ObjectId) -> Option<&Dictionary> {
    let mut id = Some(page_id);
    for _ in 0..32 {
        let page = pdf.get_dictionary(id?).ok()?;
        if let Ok((_, Object::Dictionary(resources))) =
            page.get(b"Resources").and_then(|r| pdf.dereference(r))
        {
            return Some(resources);
        }
        id = page
            .get(b"Parent")
            .ok()
            .and_then(|parent| pdf.dereference(parent).ok())
            .and_then(|(parent_id, _)| parent_id);
    }
    None
}

/// PDF user-space to the document's millimetre coordinates. PDF is already
/// Y-up; this only applies the page box origin, page rotation and unit scale.
fn page_matrix([x0, y0, x1, y1]: [f64; 4], rotation: i64, scale: f64) -> (Mat, f64, f64) {
    let width = (x1 - x0) * scale;
    let height = (y1 - y0) * scale;
    match rotation {
        90 => (
            Mat {
                a: 0.0,
                b: -scale,
                c: scale,
                d: 0.0,
                e: -y0 * scale,
                f: x1 * scale,
            },
            height,
            width,
        ),
        180 => (
            Mat {
                a: -scale,
                b: 0.0,
                c: 0.0,
                d: -scale,
                e: x1 * scale,
                f: y1 * scale,
            },
            width,
            height,
        ),
        270 => (
            Mat {
                a: 0.0,
                b: scale,
                c: -scale,
                d: 0.0,
                e: y1 * scale,
                f: -x0 * scale,
            },
            height,
            width,
        ),
        _ => (
            Mat {
                a: scale,
                b: 0.0,
                c: 0.0,
                d: scale,
                e: -x0 * scale,
                f: -y0 * scale,
            },
            width,
            height,
        ),
    }
}

#[derive(Clone, Copy, Debug)]
struct Mat {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Mat {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(self, point: Point) -> Point {
        Point::new(
            self.a * point.x + self.c * point.y + self.e,
            self.b * point.x + self.d * point.y + self.f,
        )
    }

    /// Combine a new user-space transform with the current transform.
    fn concat(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    fn from_objects(objects: &[Object]) -> Option<Self> {
        if objects.len() < 6 {
            return None;
        }
        let mut values = [0.0; 6];
        for (slot, object) in values.iter_mut().zip(objects) {
            let value = f64::from(object.as_float().ok()?);
            if !value.is_finite() {
                return None;
            }
            *slot = value;
        }
        Some(Self {
            a: values[0],
            b: values[1],
            c: values[2],
            d: values[3],
            e: values[4],
            f: values[5],
        })
    }
}

#[derive(Clone, Debug)]
struct GraphicsState {
    ctm: Mat,
    fill: [u8; 3],
    stroke: [u8; 3],
}

#[derive(Default)]
struct ActivePath {
    path: BezPath,
    current: Option<Point>,
    start: Option<Point>,
}

impl ActivePath {
    fn move_to(&mut self, point: Point) {
        self.path.move_to(point);
        self.current = Some(point);
        self.start = Some(point);
    }

    fn line_to(&mut self, point: Point) {
        if self.current.is_some() {
            self.path.line_to(point);
            self.current = Some(point);
        }
    }

    fn curve_to(&mut self, c1: Point, c2: Point, point: Point) {
        if self.current.is_some() {
            self.path.curve_to(c1, c2, point);
            self.current = Some(point);
        }
    }

    fn close(&mut self) {
        if self.current.is_some() {
            self.path.close_path();
            self.current = self.start;
        }
    }

    fn has_drawable(&self) -> bool {
        self.path
            .elements()
            .iter()
            .any(|element| !matches!(element, PathEl::MoveTo(_)))
    }

    /// Filled PDF paths close every open subpath implicitly.
    fn close_all_subpaths(&mut self) {
        if !self.has_drawable() {
            return;
        }
        let mut path = BezPath::new();
        let mut open = false;
        for element in self.path.elements() {
            match *element {
                PathEl::MoveTo(point) => {
                    if open {
                        path.close_path();
                    }
                    path.move_to(point);
                    open = false;
                }
                PathEl::LineTo(point) => {
                    path.line_to(point);
                    open = true;
                }
                PathEl::QuadTo(control, point) => {
                    path.quad_to(control, point);
                    open = true;
                }
                PathEl::CurveTo(c1, c2, point) => {
                    path.curve_to(c1, c2, point);
                    open = true;
                }
                PathEl::ClosePath => {
                    path.close_path();
                    open = false;
                }
            }
        }
        if open {
            path.close_path();
        }
        self.path = path;
        self.current = None;
        self.start = None;
    }

    fn reset(&mut self) {
        self.path = BezPath::new();
        self.current = None;
        self.start = None;
    }
}

struct PdfImporter<'a> {
    pdf: &'a PdfDocument,
    doc: Document,
    opt: &'a ImportOptions,
    next_shape: usize,
}

impl PdfImporter<'_> {
    fn parse_operations(
        &mut self,
        operations: &[Operation],
        mut state: GraphicsState,
        resources: Option<&Dictionary>,
        depth: usize,
    ) {
        if depth > MAX_FORM_DEPTH {
            return;
        }
        let mut stack: Vec<GraphicsState> = Vec::new();
        let mut path = ActivePath::default();

        for operation in operations {
            let numbers = numeric_operands(&operation.operands);
            match operation.operator.as_str() {
                "q" => stack.push(state.clone()),
                "Q" => {
                    if let Some(previous) = stack.pop() {
                        state = previous;
                    }
                }
                "cm" => {
                    if let Some(matrix) = Mat::from_objects(&operation.operands) {
                        state.ctm = state.ctm.concat(matrix);
                    }
                }
                "m" => {
                    if let Some(point) = point(&numbers, 0) {
                        path.move_to(state.ctm.apply(point));
                    }
                }
                "l" => {
                    if let Some(point) = point(&numbers, 0) {
                        path.line_to(state.ctm.apply(point));
                    }
                }
                "c" => {
                    if let (Some(c1), Some(c2), Some(point)) =
                        (point(&numbers, 0), point(&numbers, 2), point(&numbers, 4))
                    {
                        path.curve_to(
                            state.ctm.apply(c1),
                            state.ctm.apply(c2),
                            state.ctm.apply(point),
                        );
                    }
                }
                "v" => {
                    if let (Some(current), Some(c2), Some(point)) =
                        (path.current, point(&numbers, 0), point(&numbers, 2))
                    {
                        path.curve_to(current, state.ctm.apply(c2), state.ctm.apply(point));
                    }
                }
                "y" => {
                    if let (Some(c1), Some(point)) = (point(&numbers, 0), point(&numbers, 2)) {
                        path.curve_to(
                            state.ctm.apply(c1),
                            state.ctm.apply(point),
                            state.ctm.apply(point),
                        );
                    }
                }
                "h" => path.close(),
                "re" => {
                    if let (Some(x), Some(y), Some(width), Some(height)) = (
                        numbers.first().copied(),
                        numbers.get(1).copied(),
                        numbers.get(2).copied(),
                        numbers.get(3).copied(),
                    ) {
                        let p0 = state.ctm.apply(Point::new(x, y));
                        let p1 = state.ctm.apply(Point::new(x + width, y));
                        let p2 = state.ctm.apply(Point::new(x + width, y + height));
                        let p3 = state.ctm.apply(Point::new(x, y + height));
                        path.move_to(p0);
                        path.line_to(p1);
                        path.line_to(p2);
                        path.line_to(p3);
                        path.close();
                    }
                }
                "S" => self.paint(&mut path, &state, false, true, false),
                "s" => self.paint(&mut path, &state, false, true, true),
                "f" | "F" | "f*" => self.paint(&mut path, &state, true, false, false),
                "B" | "B*" => self.paint(&mut path, &state, true, true, false),
                "b" | "b*" => self.paint(&mut path, &state, true, true, true),
                "n" => path.reset(),
                "g" => set_gray(&mut state.fill, &numbers),
                "G" => set_gray(&mut state.stroke, &numbers),
                "rg" => set_rgb(&mut state.fill, &numbers),
                "RG" => set_rgb(&mut state.stroke, &numbers),
                "k" => set_cmyk(&mut state.fill, &numbers),
                "K" => set_cmyk(&mut state.stroke, &numbers),
                "sc" | "scn" => set_color(&mut state.fill, &numbers),
                "SC" | "SCN" => set_color(&mut state.stroke, &numbers),
                "Do" => self.draw_xobject(operation, &state, resources, depth),
                _ => {}
            }
        }
    }

    fn paint(
        &mut self,
        path: &mut ActivePath,
        state: &GraphicsState,
        fill: bool,
        stroke: bool,
        close: bool,
    ) {
        if close {
            path.close();
        }
        if fill {
            path.close_all_subpaths();
        }
        if path.has_drawable() {
            let color = if stroke { state.stroke } else { state.fill };
            // A full-page white fill is almost always a print background, not
            // artwork. Keep smaller white fills, which may be intentional.
            let bounds = path.path.bounding_box();
            let eps_x = (self.doc.width * 0.01).max(1.0);
            let eps_y = (self.doc.height * 0.01).max(1.0);
            let page_background = fill
                && !stroke
                && color.iter().all(|channel| *channel >= 250)
                && bounds.x0 <= eps_x
                && bounds.y0 <= eps_y
                && bounds.x1 >= self.doc.width - eps_x
                && bounds.y1 >= self.doc.height - eps_y;
            if !page_background {
                let index = layer_for(&mut self.doc, color, self.opt);
                let id = format!("pdf-path-{}", self.next_shape);
                self.next_shape += 1;
                self.doc.layers[index]
                    .shapes
                    .push(Shape::new(id, path.path.clone(), fill));
            }
        }
        path.reset();
    }

    fn draw_xobject(
        &mut self,
        operation: &Operation,
        state: &GraphicsState,
        resources: Option<&Dictionary>,
        depth: usize,
    ) {
        if depth >= MAX_FORM_DEPTH {
            return;
        }
        let Some(Object::Name(name)) = operation.operands.first() else {
            return;
        };
        let Some(resources) = resources else {
            return;
        };
        let pdf = self.pdf;
        let Ok((_, xobjects)) = resources
            .get(b"XObject")
            .and_then(|object| pdf.dereference(object))
        else {
            return;
        };
        let Ok(xobjects) = xobjects.as_dict() else {
            return;
        };
        let Ok((_, object)) = xobjects
            .get(name)
            .and_then(|object| pdf.dereference(object))
        else {
            return;
        };
        let Ok(stream) = object.as_stream() else {
            return;
        };
        let Some(subtype) = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|object| object.as_name().ok())
        else {
            return;
        };
        if subtype != b"Form".as_slice() {
            return;
        }
        let matrix = stream
            .dict
            .get(b"Matrix")
            .ok()
            .and_then(|object| pdf.dereference(object).ok())
            .and_then(|(_, object)| object.as_array().ok())
            .and_then(|objects| Mat::from_objects(objects))
            .unwrap_or(Mat::IDENTITY);
        let form_resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|object| pdf.dereference(object).ok())
            .and_then(|(_, object)| object.as_dict().ok())
            .or(Some(resources));
        let Ok(content) = decode_stream(stream) else {
            return;
        };
        let mut form_state = state.clone();
        form_state.ctm = state.ctm.concat(matrix);
        self.parse_operations(&content.operations, form_state, form_resources, depth + 1);
    }
}

fn decode_stream(stream: &Stream) -> Result<Content, lopdf::Error> {
    Content::decode(&stream.get_plain_content()?)
}

fn numeric_operands(objects: &[Object]) -> Vec<f64> {
    objects
        .iter()
        .filter_map(|object| object.as_float().ok().map(f64::from))
        .filter(|value| value.is_finite())
        .collect()
}

fn point(numbers: &[f64], offset: usize) -> Option<Point> {
    Some(Point::new(*numbers.get(offset)?, *numbers.get(offset + 1)?))
}

fn channel(value: f64) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

fn set_gray(color: &mut [u8; 3], numbers: &[f64]) {
    if let Some(gray) = numbers.first() {
        let value = channel(*gray);
        *color = [value, value, value];
    }
}

fn set_rgb(color: &mut [u8; 3], numbers: &[f64]) {
    if let (Some(r), Some(g), Some(b)) = (numbers.first(), numbers.get(1), numbers.get(2)) {
        *color = [channel(*r), channel(*g), channel(*b)];
    }
}

fn set_cmyk(color: &mut [u8; 3], numbers: &[f64]) {
    if let (Some(c), Some(m), Some(y), Some(k)) = (
        numbers.first(),
        numbers.get(1),
        numbers.get(2),
        numbers.get(3),
    ) {
        *color = [
            channel((1.0 - c) * (1.0 - k)),
            channel((1.0 - m) * (1.0 - k)),
            channel((1.0 - y) * (1.0 - k)),
        ];
    }
}

fn set_color(color: &mut [u8; 3], numbers: &[f64]) {
    match numbers.len() {
        1 => set_gray(color, numbers),
        3 => set_rgb(color, numbers),
        4 => set_cmyk(color, numbers),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream};

    fn pdf_bytes(content: &str, media: [f64; 4], rotate: i64) -> Vec<u8> {
        let mut pdf = PdfDocument::with_version("1.4");
        let pages_id = pdf.new_object_id();
        let content_id =
            pdf.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Real(media[0] as f32),
                Object::Real(media[1] as f32),
                Object::Real(media[2] as f32),
                Object::Real(media[3] as f32),
            ],
            "Contents" => content_id,
            "Resources" => Dictionary::new(),
            "Rotate" => Object::Integer(rotate),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog_id = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        pdf.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).expect("write test PDF");
        bytes
    }

    fn pdf_pages(contents: &[&str]) -> Vec<u8> {
        let mut pdf = PdfDocument::with_version("1.4");
        let pages_id = pdf.new_object_id();
        let mut page_ids = Vec::new();
        for content in contents {
            let content_id =
                pdf.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
            page_ids.push(pdf.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(100.0),
                    Object::Real(100.0),
                ],
                "Contents" => content_id,
                "Resources" => Dictionary::new(),
                "Rotate" => Object::Integer(0),
            }));
        }
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids
                    .into_iter()
                    .map(Object::Reference)
                    .collect::<Vec<_>>(),
                "Count" => contents.len() as i64,
            }),
        );
        let catalog_id = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        pdf.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).expect("write multi-page PDF");
        bytes
    }

    fn import(content: &str) -> Document {
        import_pdf(
            &pdf_bytes(content, [0.0, 0.0, 200.0, 100.0], 0),
            &ImportOptions::default(),
        )
        .expect("import PDF")
    }

    #[test]
    fn imports_stroked_paths_in_millimetres() {
        let doc = import("1 0 0 RG 2 w 10 20 m 110 20 l 110 70 l 10 70 l h S");
        assert!((doc.width - 200.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((doc.height - 100.0 * MM_PER_POINT).abs() < 1e-9);
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].color, [255, 0, 0]);
        assert_eq!(doc.shape_count(), 1);
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x0 - 10.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.y0 - 20.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.width() - 100.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.height() - 50.0 * MM_PER_POINT).abs() < 1e-9);
    }

    #[test]
    fn imports_filled_paths_and_closes_subpaths() {
        let doc = import("0 0 1 rg 10 10 m 30 10 l 30 30 l f");
        assert_eq!(doc.layers[0].color, [0, 0, 255]);
        let shape = &doc.layers[0].shapes[0];
        assert!(shape.is_fill);
        assert!(shape
            .path
            .elements()
            .iter()
            .any(|element| matches!(element, PathEl::ClosePath)));
    }

    #[test]
    fn applies_ctm_and_page_rotation() {
        let doc = import("q 2 0 0 2 10 5 cm 0 0 m 10 0 l h S Q");
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x0 - 10.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.y0 - 5.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.width() - 20.0 * MM_PER_POINT).abs() < 1e-9);

        let rotated = import_pdf(
            &pdf_bytes(
                "0 0 m 20 0 l 20 10 l 0 10 l h S",
                [0.0, 0.0, 20.0, 10.0],
                90,
            ),
            &ImportOptions::default(),
        )
        .expect("rotated PDF");
        assert!((rotated.width - 10.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((rotated.height - 20.0 * MM_PER_POINT).abs() < 1e-9);
    }

    #[test]
    fn follows_form_xobjects_with_their_matrix() {
        let mut pdf = PdfDocument::with_version("1.4");
        let pages_id = pdf.new_object_id();
        let form_id = pdf.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(20.0),
                    Object::Real(20.0),
                ],
                "Matrix" => vec![
                    Object::Real(1.0),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(1.0),
                    Object::Real(5.0),
                    Object::Real(5.0),
                ],
            },
            b"0 0 m 10 0 l 10 10 l h S".to_vec(),
        ));
        let content_id = pdf.add_object(Stream::new(Dictionary::new(), b"q /Fm0 Do Q".to_vec()));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(100.0),
                Object::Real(100.0),
            ],
            "Contents" => content_id,
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Fm0" => form_id },
            },
            "Rotate" => Object::Integer(0),
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog_id = pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        pdf.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).expect("write form PDF");
        let doc = import_pdf(&bytes, &ImportOptions::default()).expect("import form PDF");
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x0 - 5.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.y0 - 5.0 * MM_PER_POINT).abs() < 1e-9);
        assert!((bounds.width() - 10.0 * MM_PER_POINT).abs() < 1e-9);
    }

    #[test]
    fn skips_full_page_white_backgrounds() {
        let doc = import("1 1 1 rg 0 0 200 100 re f 1 0 0 RG 10 10 m 20 10 l 20 20 l h S");
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].color, [255, 0, 0]);
        assert_eq!(doc.shape_count(), 1);
    }

    #[test]
    fn selects_a_pdf_page() {
        let bytes = pdf_pages(&[
            "1 0 0 RG 10 10 m 20 10 l h S",
            "0 0 1 RG 20 20 m 30 20 l h S",
        ]);
        assert_eq!(pdf_page_count(&bytes).expect("page count"), 2);

        let options = ImportOptions {
            pdf_page: 1,
            ..ImportOptions::default()
        };
        let doc = import_pdf(&bytes, &options).expect("second page");
        assert_eq!(doc.layers[0].color, [0, 0, 255]);
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x0 - 20.0 * MM_PER_POINT).abs() < 1e-9);

        let options = ImportOptions {
            pdf_page: 2,
            ..options
        };
        assert!(import_pdf(&bytes, &options).is_err());
    }

    #[test]
    fn rejects_files_without_pdf_header() {
        let error = import_pdf(b"not a PDF", &ImportOptions::default()).expect_err("reject");
        assert!(matches!(error, ImportError::Pdf(_)));
    }
}
