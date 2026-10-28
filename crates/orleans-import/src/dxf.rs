//! Minimal ASCII DXF import.
//!
//! Supports the common 2D entities (`LINE`, `LWPOLYLINE`, `POLYLINE`,
//! `CIRCLE`, `ARC`, `ELLIPSE`, `SPLINE`) and the `$INSUNITS` header variable.
//! Blocks, hatches, text, dimensions and binary DXF are not expanded.

use std::collections::BTreeMap;

use kurbo::{Arc, BezPath, Ellipse, Point, Shape as _, Vec2};
use orleans_core::{Document, Layer, LayerMode, Shape};

use crate::{ImportError, ImportOptions};

const DXF_TOLERANCE: f64 = 0.02;

#[derive(Clone, Debug)]
struct Pair {
    code: i32,
    value: String,
}

impl Pair {
    fn number(&self) -> Option<f64> {
        self.value.trim().parse().ok()
    }
}

#[derive(Clone, Debug)]
struct Entity {
    kind: String,
    fields: Vec<Pair>,
}

impl Entity {
    fn text(&self, code: i32) -> Option<&str> {
        self.fields
            .iter()
            .find(|pair| pair.code == code)
            .map(|pair| pair.value.trim())
    }

    fn number(&self, code: i32) -> Option<f64> {
        self.fields
            .iter()
            .find(|pair| pair.code == code)
            .and_then(Pair::number)
    }

    fn integer(&self, code: i32) -> Option<i64> {
        self.text(code)?.parse().ok()
    }

    fn point(&self, x_code: i32, y_code: i32) -> Option<Point> {
        Some(Point::new(self.number(x_code)?, self.number(y_code)?))
    }

    fn color(&self) -> Option<[u8; 3]> {
        if let Some(true_color) = self.integer(420) {
            return Some(true_color_color(true_color));
        }
        self.integer(62).and_then(|index| match index {
            0 | 256 => None,
            value => Some(aci_color(value as i32)),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    point: Point,
    bulge: f64,
}

struct Accumulator {
    names: Vec<String>,
    shapes: BTreeMap<String, Vec<Shape>>,
    colors: BTreeMap<String, [u8; 3]>,
    counts: BTreeMap<String, usize>,
}

impl Accumulator {
    fn new() -> Self {
        Self {
            names: Vec::new(),
            shapes: BTreeMap::new(),
            colors: BTreeMap::new(),
            counts: BTreeMap::new(),
        }
    }

    fn add(&mut self, layer: &str, color: [u8; 3], path: BezPath, is_fill: bool) {
        let name = if layer.is_empty() { "0" } else { layer };
        if !self.shapes.contains_key(name) {
            self.names.push(name.to_owned());
            self.shapes.insert(name.to_owned(), Vec::new());
            self.colors.insert(name.to_owned(), color);
        }
        let count = self.counts.entry(name.to_owned()).or_insert(0);
        let id = format!("dxf-{name}-{count}");
        *count += 1;
        self.shapes
            .get_mut(name)
            .expect("layer exists")
            .push(Shape::new(id, path, is_fill));
    }
}

/// Parse ASCII DXF bytes into a document in millimetres.
pub fn import_dxf(data: &[u8], opt: &ImportOptions) -> Result<Document, ImportError> {
    let text = String::from_utf8_lossy(data);
    let pairs = parse_pairs(&text);
    if pairs.is_empty() {
        return Err(ImportError::Dxf("empty or binary DXF".to_owned()));
    }
    let unit_scale = units_to_mm(header_integer(&pairs, "$INSUNITS"))
        * if opt.scale.is_finite() && opt.scale > 0.0 {
            opt.scale
        } else {
            1.0
        };
    let colors = parse_layers(&pairs);
    let records = entities(&pairs);
    let mut accumulator = Accumulator::new();

    let mut index = 0;
    while index < records.len() {
        let entity = &records[index];
        let layer = entity.text(8).unwrap_or("0");
        let color = entity
            .color()
            .or_else(|| colors.get(layer).copied())
            .unwrap_or([0, 0, 0]);
        match entity.kind.as_str() {
            "LINE" => {
                if let (Some(start), Some(end)) = (entity.point(10, 20), entity.point(11, 21)) {
                    let mut path = BezPath::new();
                    path.move_to(scale_point(start, unit_scale));
                    path.line_to(scale_point(end, unit_scale));
                    accumulator.add(layer, color, path, false);
                }
            }
            "LWPOLYLINE" => {
                let closed = entity.integer(70).is_some_and(|flags| flags & 1 != 0);
                if let Some(path) =
                    polyline_path(&lw_vertices(entity), closed, unit_scale, DXF_TOLERANCE)
                {
                    accumulator.add(layer, color, path, false);
                }
            }
            "POLYLINE" => {
                let closed = entity.integer(70).is_some_and(|flags| flags & 1 != 0);
                let mut vertices = Vec::new();
                let mut next = index + 1;
                while next < records.len() {
                    match records[next].kind.as_str() {
                        "VERTEX" => {
                            if let (Some(x), Some(y)) =
                                (records[next].number(10), records[next].number(20))
                            {
                                vertices.push(Vertex {
                                    point: Point::new(x, y),
                                    bulge: records[next].number(42).unwrap_or(0.0),
                                });
                            }
                            next += 1;
                        }
                        "SEQEND" => {
                            next += 1;
                            break;
                        }
                        _ => break,
                    }
                }
                if let Some(path) = polyline_path(&vertices, closed, unit_scale, DXF_TOLERANCE) {
                    accumulator.add(layer, color, path, false);
                }
                index = next;
                continue;
            }
            "CIRCLE" => {
                if let (Some(center), Some(radius)) = (entity.point(10, 20), entity.number(40)) {
                    if radius > 1e-9 {
                        let ellipse = Ellipse::new(
                            scale_point(center, unit_scale),
                            Vec2::new(radius * unit_scale, radius * unit_scale),
                            0.0,
                        );
                        accumulator.add(layer, color, ellipse.to_path(DXF_TOLERANCE), false);
                    }
                }
            }
            "ARC" => {
                if let (Some(center), Some(radius), Some(start), Some(end)) = (
                    entity.point(10, 20),
                    entity.number(40),
                    entity.number(50),
                    entity.number(51),
                ) {
                    if radius > 1e-9 {
                        let start = start.to_radians();
                        let mut end = end.to_radians();
                        if end <= start {
                            end += std::f64::consts::TAU;
                        }
                        let arc = Arc::new(
                            scale_point(center, unit_scale),
                            Vec2::new(radius * unit_scale, radius * unit_scale),
                            start,
                            end - start,
                            0.0,
                        );
                        accumulator.add(layer, color, arc.to_path(DXF_TOLERANCE), false);
                    }
                }
            }
            "ELLIPSE" => {
                if let (Some(center), Some(major)) = (entity.point(10, 20), entity.point(11, 21)) {
                    let major_length = (major - Point::ORIGIN).hypot();
                    let ratio = entity.number(40).unwrap_or(1.0).clamp(1e-6, 1.0);
                    if major_length > 1e-9 {
                        let start = entity.number(41).unwrap_or(0.0);
                        let mut end = entity.number(42).unwrap_or(std::f64::consts::TAU);
                        if end <= start {
                            end += std::f64::consts::TAU;
                        }
                        let rotation = major.y.atan2(major.x);
                        let center = scale_point(center, unit_scale);
                        let radii =
                            Vec2::new(major_length * unit_scale, major_length * ratio * unit_scale);
                        let path = if (end - start) >= std::f64::consts::TAU - 1e-9 {
                            Ellipse::new(center, radii, rotation).to_path(DXF_TOLERANCE)
                        } else {
                            Arc::new(center, radii, start, end - start, rotation)
                                .to_path(DXF_TOLERANCE)
                        };
                        accumulator.add(layer, color, path, false);
                    }
                }
            }
            "SPLINE" => {
                if let Some(path) = spline_path(entity, unit_scale) {
                    accumulator.add(layer, color, path, false);
                }
            }
            _ => {}
        }
        index += 1;
    }

    let mut doc = Document::new(1.0, 1.0);
    for (index, name) in accumulator.names.iter().enumerate() {
        let color = accumulator.colors.get(name).copied().unwrap_or([0, 0, 0]);
        let mut layer = Layer::new(index, name.clone(), color, opt.default_cut);
        layer.mode = LayerMode::Line;
        layer.shapes = accumulator.shapes.remove(name).unwrap_or_default();
        doc.layers.push(layer);
    }
    if let Some(bounds) = doc.bounds() {
        doc.width = bounds.x1.max(1.0);
        doc.height = bounds.y1.max(1.0);
    }
    Ok(doc)
}

fn parse_pairs(text: &str) -> Vec<Pair> {
    let mut pairs = Vec::new();
    let mut lines = text.lines();
    while let (Some(code), Some(value)) = (lines.next(), lines.next()) {
        let Ok(code) = code.trim().parse::<i32>() else {
            continue;
        };
        pairs.push(Pair {
            code,
            value: value.trim().to_owned(),
        });
    }
    pairs
}

fn section<'a>(pairs: &'a [Pair], name: &str) -> Option<&'a [Pair]> {
    let mut index = 0;
    while index + 1 < pairs.len() {
        if pairs[index].code == 0
            && pairs[index].value == "SECTION"
            && pairs[index + 1].code == 2
            && pairs[index + 1].value == name
        {
            let start = index + 2;
            let end = pairs[start..]
                .iter()
                .position(|pair| pair.code == 0 && pair.value == "ENDSEC")
                .map(|offset| start + offset)
                .unwrap_or(pairs.len());
            return Some(&pairs[start..end]);
        }
        index += 1;
    }
    None
}

fn header_integer(pairs: &[Pair], name: &str) -> Option<i32> {
    let header = section(pairs, "HEADER")?;
    let mut index = 0;
    while index + 1 < header.len() {
        if header[index].code == 9 && header[index].value == name {
            return header[index + 1].text_parse();
        }
        index += 1;
    }
    None
}

impl Pair {
    fn text_parse<T: std::str::FromStr>(&self) -> Option<T> {
        self.value.trim().parse().ok()
    }
}

fn parse_layers(pairs: &[Pair]) -> BTreeMap<String, [u8; 3]> {
    let mut colors = BTreeMap::new();
    let Some(table) = section(pairs, "TABLES") else {
        return colors;
    };
    let mut index = 0;
    while index < table.len() {
        if table[index].code == 0 && table[index].value == "LAYER" {
            let mut name = None;
            let mut color = [0, 0, 0];
            index += 1;
            while index < table.len() && table[index].code != 0 {
                match table[index].code {
                    2 => name = Some(table[index].value.clone()),
                    62 => {
                        if let Ok(value) = table[index].value.parse::<i32>() {
                            color = aci_color(value);
                        }
                    }
                    420 => {
                        if let Ok(value) = table[index].value.parse::<i64>() {
                            color = true_color_color(value);
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
            if let Some(name) = name {
                colors.insert(name, color);
            }
        } else {
            index += 1;
        }
    }
    colors
}

fn entities(pairs: &[Pair]) -> Vec<Entity> {
    let Some(section) = section(pairs, "ENTITIES") else {
        return Vec::new();
    };
    let mut entities = Vec::new();
    let mut current: Option<Entity> = None;
    for pair in section {
        if pair.code == 0 {
            if let Some(entity) = current.take() {
                entities.push(entity);
            }
            current = Some(Entity {
                kind: pair.value.clone(),
                fields: Vec::new(),
            });
        } else if let Some(entity) = current.as_mut() {
            entity.fields.push(pair.clone());
        }
    }
    if let Some(entity) = current {
        entities.push(entity);
    }
    entities
}

fn lw_vertices(entity: &Entity) -> Vec<Vertex> {
    let mut vertices: Vec<Vertex> = Vec::new();
    for pair in &entity.fields {
        match pair.code {
            10 => {
                if let Some(x) = pair.number() {
                    vertices.push(Vertex {
                        point: Point::new(x, 0.0),
                        bulge: 0.0,
                    });
                }
            }
            20 => {
                if let (Some(y), Some(vertex)) = (pair.number(), vertices.last_mut()) {
                    vertex.point.y = y;
                }
            }
            42 => {
                if let (Some(bulge), Some(vertex)) = (pair.number(), vertices.last_mut()) {
                    vertex.bulge = bulge;
                }
            }
            _ => {}
        }
    }
    vertices
}

fn polyline_path(vertices: &[Vertex], closed: bool, scale: f64, tolerance: f64) -> Option<BezPath> {
    if vertices.len() < 2 {
        return None;
    }
    let mut path = BezPath::new();
    path.move_to(scale_point(vertices[0].point, scale));
    for index in 1..vertices.len() {
        append_segment(
            &mut path,
            scale_point(vertices[index - 1].point, scale),
            scale_point(vertices[index].point, scale),
            vertices[index - 1].bulge,
            tolerance,
        );
    }
    if closed {
        append_segment(
            &mut path,
            scale_point(vertices[vertices.len() - 1].point, scale),
            scale_point(vertices[0].point, scale),
            vertices[vertices.len() - 1].bulge,
            tolerance,
        );
        path.close_path();
    }
    Some(path)
}

fn append_segment(path: &mut BezPath, start: Point, end: Point, bulge: f64, tolerance: f64) {
    let chord = (end - start).hypot();
    if bulge.abs() < 1e-9 || chord < 1e-12 {
        path.line_to(end);
        return;
    }
    let theta = 4.0 * bulge.atan();
    let half = chord * 0.5;
    let height = half / (theta * 0.5).tan();
    let mid = Point::new((start.x + end.x) * 0.5, (start.y + end.y) * 0.5);
    let left = Vec2::new(-(end.y - start.y) / chord, (end.x - start.x) / chord);
    let center = Point::new(mid.x + left.x * height, mid.y + left.y * height);
    let radius = (start - center).hypot();
    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let arc = Arc::new(center, Vec2::new(radius, radius), start_angle, theta, 0.0);
    path.extend(arc.to_path(tolerance).elements().iter().skip(1).copied());
}

fn scale_point(point: Point, scale: f64) -> Point {
    Point::new(point.x * scale, point.y * scale)
}

fn spline_path(entity: &Entity, scale: f64) -> Option<BezPath> {
    let mut control = Vec::new();
    let mut fit = Vec::new();
    let mut x = None;
    for pair in &entity.fields {
        match pair.code {
            10 => {
                if let Some(value) = pair.number() {
                    x = Some(value);
                }
            }
            20 => {
                if let (Some(value), Some(y)) = (x, pair.number()) {
                    control.push(Point::new(value, y));
                    x = None;
                }
            }
            11 => {
                if let Some(value) = pair.number() {
                    x = Some(value);
                }
            }
            21 => {
                if let (Some(value), Some(y)) = (x, pair.number()) {
                    fit.push(Point::new(value, y));
                    x = None;
                }
            }
            _ => {}
        }
    }
    let closed = entity.integer(70).is_some_and(|flags| flags & 1 != 0);
    let degree = entity.integer(71).unwrap_or(3).clamp(1, 10) as usize;
    let knots: Vec<f64> = entity
        .fields
        .iter()
        .filter(|pair| pair.code == 40)
        .filter_map(Pair::number)
        .collect();

    let points = if control.len() >= 2 { control } else { fit };
    if points.len() < 2 {
        return None;
    }
    let points: Vec<Point> = points
        .into_iter()
        .map(|point| scale_point(point, scale))
        .collect();

    let mut path = BezPath::new();
    if knots.len() > points.len() + degree {
        let first = knots[degree];
        let last = knots[points.len()];
        if first.is_finite() && last > first {
            let samples = (points.len() * 8).clamp(16, 2000);
            for sample in 0..=samples {
                let t = first + (last - first) * sample as f64 / samples as f64;
                let point = de_boor(&points, &knots, degree, t);
                if sample == 0 {
                    path.move_to(point);
                } else {
                    path.line_to(point);
                }
            }
        } else {
            path.move_to(points[0]);
            for point in &points[1..] {
                path.line_to(*point);
            }
        }
    } else {
        path.move_to(points[0]);
        for point in &points[1..] {
            path.line_to(*point);
        }
    }
    if closed {
        path.close_path();
    }
    Some(path)
}

fn de_boor(points: &[Point], knots: &[f64], degree: usize, t: f64) -> Point {
    let n = points.len();
    let mut k = degree;
    while k + 1 < knots.len() && k + 1 < n && t >= knots[k + 1] {
        k += 1;
    }
    let mut work: Vec<Point> = (0..=degree)
        .map(|index| points[(k - degree + index).min(n - 1)])
        .collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let denominator = knots[k + j - r + 1] - knots[k + j - degree];
            let alpha = if denominator.abs() < 1e-12 {
                0.0
            } else {
                (t - knots[k + j - degree]) / denominator
            };
            work[j] = Point::new(
                work[j - 1].x * (1.0 - alpha) + work[j].x * alpha,
                work[j - 1].y * (1.0 - alpha) + work[j].y * alpha,
            );
        }
    }
    work[degree]
}

fn units_to_mm(units: Option<i32>) -> f64 {
    match units.unwrap_or(4) {
        0 | 4 => 1.0,
        1 => 25.4,
        2 => 304.8,
        3 => 1_609_344.0,
        5 => 10.0,
        6 => 1000.0,
        7 => 1_000_000.0,
        8 => 0.000_025_4,
        9 => 0.025_4,
        10 => 914.4,
        11 => 1e-7,
        12 => 1e-6,
        13 => 0.001,
        14 => 100.0,
        15 => 10_000.0,
        16 => 100_000.0,
        _ => 1.0,
    }
}

fn true_color_color(value: i64) -> [u8; 3] {
    [
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ]
}

fn aci_color(index: i32) -> [u8; 3] {
    match index.abs() {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 => [0, 0, 0],
        8 => [128, 128, 128],
        9 => [192, 192, 192],
        value if value > 9 => hsv_color(((value as f64) * 137.508) % 360.0),
        _ => [0, 0, 0],
    }
}

fn hsv_color(hue: f64) -> [u8; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = 0.72;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 0.25;
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(units: i32) -> String {
        format!(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{units}\n0\nENDSEC\n\
             0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n0\nLAYER\n2\nCUT\n62\n1\n0\nENDTAB\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\nCUT\n10\n0\n20\n0\n11\n10\n21\n0\n\
             0\nCIRCLE\n8\nCUT\n10\n5\n20\n5\n40\n1\n\
             0\nLWPOLYLINE\n8\nCUT\n90\n4\n70\n1\n10\n0\n20\n0\n10\n2\n20\n0\n10\n2\n20\n2\n10\n0\n20\n2\n\
             0\nPOLYLINE\n8\nCUT\n70\n1\n0\nVERTEX\n8\nCUT\n10\n10\n20\n10\n0\nVERTEX\n8\nCUT\n10\n12\n20\n10\n0\nSEQEND\n\
             0\nARC\n8\nCUT\n10\n0\n20\n0\n40\n5\n50\n0\n51\n90\n\
             0\nENDSEC\n0\nEOF\n"
        )
    }

    #[test]
    fn imports_common_dxf_entities_into_layers() {
        let doc = import_dxf(sample(4).as_bytes(), &ImportOptions::default()).expect("dxf");
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].name, "CUT");
        assert_eq!(doc.layers[0].color, [255, 0, 0]);
        assert_eq!(doc.shape_count(), 5);
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x1 - 12.0).abs() < 0.05);
        assert!((bounds.y1 - 10.0).abs() < 0.05);
        assert!((doc.width - 12.0).abs() < 0.05);
        assert!((doc.height - 10.0).abs() < 0.05);
    }

    #[test]
    fn header_units_scale_the_geometry() {
        let doc = import_dxf(sample(1).as_bytes(), &ImportOptions::default()).expect("dxf");
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.x1 - 12.0 * 25.4).abs() < 0.1);
        assert!((doc.width - 12.0 * 25.4).abs() < 0.1);
    }

    #[test]
    fn spline_control_points_are_evaluated() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nSPLINE\n8\n0\n70\n0\n71\n3\n72\n8\n73\n4\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n10\n0\n20\n0\n10\n0\n20\n10\n10\n10\n20\n10\n10\n10\n20\n0\n0\nENDSEC\n0\nEOF\n";
        let doc = import_dxf(dxf.as_bytes(), &ImportOptions::default()).expect("dxf");
        assert_eq!(doc.shape_count(), 1);
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.width() - 10.0).abs() < 0.1);
        assert!((bounds.height() - 7.5).abs() < 0.2);
    }

    #[test]
    fn polyline_bulges_become_arcs() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n2\n70\n0\n10\n0\n20\n0\n42\n1\n10\n10\n20\n0\n0\nENDSEC\n0\nEOF\n";
        let doc = import_dxf(dxf.as_bytes(), &ImportOptions::default()).expect("dxf");
        let bounds = doc.bounds().expect("bounds");
        assert!((bounds.width() - 10.0).abs() < 0.05);
        assert!((bounds.height() - 5.0).abs() < 0.05);
    }
}
